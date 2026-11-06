use crate::*;
use gpui::{
    AppContext, Bounds, Context, Empty, IntoElement, Modifiers, MouseButton, Pixels, Render,
    TestAppContext, VisualTestContext, Window, canvas, div, point, prelude::*, px, rgb, size,
};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

// Deliberately distinct raised/surface tokens catch accidental idle header seams.
const STYLE: ControlStyle = ControlStyle {
    background: 0x191919,
    surface: 0x474747,
    raised: 0xabcdef,
    rail: 0x363636,
    border: 0x474747,
    text: 0xeeeeee,
    muted: 0xaaaaaa,
    accent: 0x00ffff,
    accent_text: 0x202020,
    numeric_fill: 0x31515a,
    hover_text: 0xe0e0e0,
    focus_marker: 0xaaaaaa,
    dial_rail: 0,
    hover: 0x555555,
    selection: 0x3350a0,
    invalid: 0xff554c,
    font_size: 9.0,
    text_case: crate::TextCase::Preserve,
};

type Masks = Rc<RefCell<Vec<(&'static str, Bounds<Pixels>)>>>;
fn probe(name: &'static str, masks: &Masks) -> impl IntoElement {
    let masks = masks.clone();
    canvas(
        |_, _, _| (),
        move |_, _, window, _| {
            masks
                .borrow_mut()
                .push((name, window.content_mask().bounds));
        },
    )
    .absolute()
    .size_full()
}

struct FrameFixture {
    masks: Masks,
    clicks: Rc<Cell<usize>>,
}
impl Render for FrameFixture {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        assert_painted_styles(window, cx);
        let clicks = self.clicks.clone();
        let oversized = div()
            .absolute()
            .left(px(-20.0))
            .top(px(-20.0))
            .size(px(2000.0))
            .bg(rgb(0xff0000))
            .on_mouse_down(MouseButton::Left, move |_, _, _| {
                clicks.set(clicks.get() + 1)
            })
            .child(probe("section", &self.masks));
        device_frame(
            "long title that must remain bounded",
            STYLE,
            div(),
            div()
                .w(px(120.0))
                .h(px(60.0))
                .flex_shrink_0()
                .bg(rgb(0x00ff00))
                .child(probe("header", &self.masks)),
            device_body(STYLE)
                .id("body")
                .debug_selector(|| "body".into())
                .flex_1()
                .child(probe("body", &self.masks))
                .child(
                    device_display(STYLE)
                        .id("section")
                        .debug_selector(|| "section".into())
                        .size_full()
                        .child(oversized),
                ),
        )
        .id("frame")
        .debug_selector(|| "frame".into())
        .size_full()
    }
}

#[gpui::test]
fn rendered_shell_and_rounded_sections_clip_oversized_artwork_and_hits(cx: &mut TestAppContext) {
    let masks: Masks = Rc::new(RefCell::new(Vec::new()));
    let clicks = Rc::new(Cell::new(0));
    let bootstrap = cx.add_window(|_, _| Empty);
    let handle = *bootstrap;
    cx.update_window(handle, |_, window, cx| {
        window.replace_root(cx, |_, _| FrameFixture {
            masks: masks.clone(),
            clicks: clicks.clone(),
        });
    })
    .expect("window");
    let mut visual = VisualTestContext::from_window(handle, cx);
    for (width, height) in [(420.0, 160.0), (120.0, 68.0)] {
        masks.borrow_mut().clear();
        visual.simulate_resize(size(px(width), px(height)));
        visual.run_until_parked();
        let frame = visual.debug_bounds("frame").expect("frame");
        let header = visual.debug_bounds("device-frame-header").expect("header");
        let body = visual.debug_bounds("body").expect("body");
        let section = visual.debug_bounds("section").expect("section");
        let scale = visual.update(|window, _| window.scale_factor());
        for (selector, radius) in [
            ("frame", DEVICE_CORNER_RADIUS),
            ("device-frame-header", DEVICE_SECTION_RADIUS),
            ("section", DEVICE_SECTION_RADIUS),
        ] {
            let (corners, _) = visual
                .rendered_surface_geometry(selector)
                .expect("filled rounded quad");
            assert_eq!(corners.top_left, px(radius).scale(scale));
        }
        // Every surface also paints a border-only quad after its children. The
        // accessor must select the actual filled background instead of that quad.
        assert_eq!(
            visual.rendered_background("frame"),
            visual.rendered_background("device-frame-header")
        );
        assert_eq!(
            visual.rendered_background("device-frame-header"),
            Some(gpui::Background::from(rgb(STYLE.surface)))
        );
        assert_eq!(
            visual.rendered_background("section"),
            Some(gpui::Background::from(rgb(STYLE.background)))
        );

        assert_eq!(header.size.width, px(DEVICE_FRAME_HEADER_WIDTH));
        assert_eq!(
            header.origin.x,
            frame.origin.x + px(DEVICE_FRAME_BORDER_WIDTH)
        );
        assert_eq!(
            header.origin.y,
            frame.origin.y + px(DEVICE_FRAME_BORDER_WIDTH)
        );
        assert_eq!(body.origin.y, header.origin.y);
        assert_eq!(body.origin.x, header.right());
        assert_eq!(header.size.height, body.size.height);
        assert_eq!(
            body.bottom_left().y,
            frame.bottom_left().y - px(DEVICE_FRAME_BORDER_WIDTH)
        );
        assert_eq!(section.size.height, body.size.height - px(2.0));
        for (name, bounds, inset) in [
            ("header", header, DEVICE_FRAME_HEADER_PADDING),
            ("body", body, DEVICE_BODY_INSET),
            ("section", section, DEVICE_SECTION_INSET),
        ] {
            let mask = masks
                .borrow()
                .iter()
                .rev()
                .find(|(label, _)| *label == name)
                .expect("paint ran")
                .1;
            assert_eq!(mask.origin.x, bounds.origin.x + px(inset), "{name}");
            assert_eq!(
                mask.size.width,
                bounds.size.width - px(inset * 2.0),
                "{name}"
            );
            assert!(
                mask.origin.y >= bounds.origin.y && mask.bottom_left().y <= bounds.bottom_left().y
            );
            // A rectangular mask wholly inside the rounded region excludes every
            // corner cutout; no GPU framebuffer/readback claim is made here.
            assert!(inset >= DEVICE_SECTION_RADIUS);
        }
        let before = clicks.get();
        visual.simulate_click(
            point(section.origin.x + px(1.0), section.origin.y + px(1.0)),
            Modifiers::default(),
        );
        visual.run_until_parked();
        assert_eq!(
            clicks.get(),
            before,
            "rounded corner excludes oversized child hitbox"
        );
        visual.simulate_click(section.center(), Modifiers::default());
        visual.run_until_parked();
        assert_eq!(clicks.get(), before + 1, "interior remains operable");
    }
}

fn assert_painted_styles(window: &mut Window, cx: &mut gpui::App) {
    let mut frame = device_frame("title", STYLE, div(), div(), div());
    let mut header = device_header("title", STYLE, div(), div());
    let mut section = device_display(STYLE);
    let mut row = device_grouped_row(STYLE, false);
    let frame = frame.interactivity().compute_style(None, None, window, cx);
    let header = header.interactivity().compute_style(None, None, window, cx);
    let section = section
        .interactivity()
        .compute_style(None, None, window, cx);
    let row = row.interactivity().compute_style(None, None, window, cx);
    assert_eq!(
        frame.background, header.background,
        "idle header and surround share injected surface"
    );
    assert_eq!(header.border_widths.top, px(0.0).into());
    assert_eq!(
        header.border_widths.bottom,
        px(0.0).into(),
        "no separating rule"
    );
    assert_eq!(frame.corner_radii.top_left, px(DEVICE_CORNER_RADIUS).into());
    assert_eq!(
        header.corner_radii.top_left,
        px(DEVICE_SECTION_RADIUS).into()
    );
    assert_eq!(
        section.corner_radii.bottom_right,
        px(DEVICE_SECTION_RADIUS).into()
    );
    assert_eq!(row.corner_radii.top_left, section.corner_radii.top_left);
}

struct LayoutFixture;
impl Render for LayoutFixture {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let tile = |name: &'static str| {
            div()
                .id(name)
                .debug_selector(move || name.into())
                .w(px(48.0))
                .h(px(40.0))
                .flex_shrink_0()
        };
        device_layout(DeviceLayout::Column)
            .size_full()
            .child(
                device_rail(STYLE).w_full().child(
                    device_grouped_row(STYLE, false)
                        .h(px(40.0))
                        .child(tile("short")),
                ),
            )
            .child(
                device_layout(DeviceLayout::Wrapped)
                    .w_full()
                    .child(tile("wrap-a"))
                    .child(tile("wrap-b"))
                    .child(tile("wrap-c")),
            )
            .child(
                device_layout(DeviceLayout::Grid(2))
                    .w_full()
                    .child(tile("grid-a"))
                    .child(tile("grid-b"))
                    .child(tile("grid-c")),
            )
            .child(device_split(tile("split-a"), tile("split-b")))
    }
}

#[gpui::test]
fn rendered_compositions_reflow_without_changing_control_heights(cx: &mut TestAppContext) {
    let handle = *cx.add_window(|_, _| LayoutFixture);
    let mut visual = VisualTestContext::from_window(handle, cx);
    for width in [360.0, 120.0] {
        visual.simulate_resize(size(px(width), px(400.0)));
        visual.run_until_parked();
        let bounds =
            |visual: &mut VisualTestContext, name| visual.debug_bounds(name).expect("tile");
        let a = bounds(&mut visual, "wrap-a");
        let b = bounds(&mut visual, "wrap-b");
        let c = bounds(&mut visual, "wrap-c");
        assert_eq!(a.origin.y, b.origin.y);
        assert_eq!(c.origin.y > b.origin.y, width < 200.0);
        let ga = bounds(&mut visual, "grid-a");
        let gb = bounds(&mut visual, "grid-b");
        let gc = bounds(&mut visual, "grid-c");
        assert_eq!(ga.origin.y, gb.origin.y);
        assert!(gc.origin.y >= ga.bottom_left().y);
        let sa = bounds(&mut visual, "split-a");
        let sb = bounds(&mut visual, "split-b");
        assert!(sa.bottom_right().x <= sb.origin.x);
        for tile in [a, b, c, ga, gb, gc, sa, sb, bounds(&mut visual, "short")] {
            assert_eq!(tile.size.height, px(40.0));
            assert!(tile.origin.x >= px(0.0) && tile.bottom_right().x <= px(width));
        }
    }
}

struct SelectionFixture {
    selected: bool,
}
impl Render for SelectionFixture {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut frame = device_frame(
            "device",
            STYLE.with_device_selection(self.selected),
            div(),
            div(),
            device_body(STYLE)
                .id("selection-body")
                .debug_selector(|| "selection-body".into())
                .flex_1()
                .child(
                    device_display(STYLE)
                        .id("selection-section")
                        .debug_selector(|| "selection-section".into())
                        .size_full(),
                ),
        )
        .id("selection-frame")
        .debug_selector(|| "selection-frame".into())
        .size_full();
        // Filled paint is checked from the completed scene below. Resolve border
        // style during rendering too; this does not claim GPU pixel readback.
        let mut idle = device_frame("device", STYLE, div(), div(), div());
        let idle = idle.interactivity().compute_style(None, None, window, cx);
        let current = frame.interactivity().compute_style(None, None, window, cx);
        assert_eq!(current.border_color, idle.border_color);
        assert_eq!(current.border_widths, idle.border_widths);
        frame
    }
}

#[gpui::test]
fn selection_changes_only_header_paint_without_moving_or_recoloring_the_frame(
    cx: &mut TestAppContext,
) {
    let handle = *cx.add_window(|_, _| SelectionFixture { selected: false });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(220.0), px(120.0)));
    visual.run_until_parked();
    let selectors = ["selection-frame", "selection-body", "selection-section"];
    let baseline: Vec<_> = selectors
        .iter()
        .map(|selector| {
            (
                visual.debug_bounds(selector).expect("rendered region"),
                visual.rendered_background(selector),
                visual.rendered_surface_geometry(selector),
            )
        })
        .collect();
    for selected in [true, false] {
        visual.update(|window, app| {
            window
                .root::<SelectionFixture>()
                .expect("window root")
                .expect("selection fixture")
                .update(app, |view, cx| {
                    view.selected = selected;
                    cx.notify();
                });
        });
        visual.run_until_parked();
        for (selector, expected) in selectors.iter().zip(&baseline) {
            assert_eq!(
                visual.debug_bounds(selector),
                Some(expected.0),
                "{selector}"
            );
            assert_eq!(
                visual.rendered_background(selector),
                expected.1,
                "{selector}"
            );
            assert_eq!(
                visual.rendered_surface_geometry(selector),
                expected.2,
                "{selector}"
            );
        }
        assert_eq!(
            visual.rendered_background("device-frame-header"),
            Some(gpui::Background::from(rgb(if selected {
                STYLE.accent
            } else {
                STYLE.surface
            })))
        );
    }
}

struct CompactSectionFixture {
    masks: Masks,
}
impl Render for CompactSectionFixture {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let mut column = device_layout(DeviceLayout::Column).gap(px(2.0));
        for name in ["dense-first", "dense-middle", "dense-last"] {
            column = column.child(
                div()
                    .debug_selector(move || name.into())
                    .w(px(64.0))
                    .h(px(52.0))
                    .flex_shrink_0()
                    .bg(rgb(0x112233)),
            );
        }
        div()
            .flex()
            .gap(px(8.0))
            .child(
                device_compact_section(STYLE, DeviceSectionSurface::Rail)
                    .debug_selector(|| "vertical-section".into())
                    .w(px(64.0))
                    .h(px(162.0))
                    .child(probe("vertical", &self.masks))
                    .child(
                        div()
                            .debug_selector(|| "full-width-control".into())
                            .w(px(64.0))
                            .h(px(52.0))
                            .bg(rgb(0x112233)),
                    ),
            )
            .child(
                device_dense_section(STYLE, DeviceSectionSurface::Rail)
                    .debug_selector(|| "dense-section".into())
                    .w(px(64.0))
                    .h(px(162.0))
                    .child(probe("dense", &self.masks))
                    .child(column),
            )
            .child(
                device_compact_row(STYLE, true)
                    .debug_selector(|| "short-row".into())
                    .w(px(92.0))
                    .h(px(17.0))
                    .child(
                        div()
                            .debug_selector(|| "short-control".into())
                            .w(px(64.0))
                            .h(px(14.0)),
                    ),
            )
            .child(
                device_action(
                    "×".into(),
                    ActionStyle {
                        background: 0x123456,
                        text: 0xffffff,
                        hover_background: 0xabcdef,
                        hover_text: Some(0x000000),
                    },
                )
                .id("shared-action")
                .debug_selector(|| "shared-action".into()),
            )
    }
}

#[gpui::test]
fn vertical_and_dense_sections_preserve_control_widths_heights_and_action_hover(
    cx: &mut TestAppContext,
) {
    let masks: Masks = Rc::new(RefCell::new(Vec::new()));
    let handle = cx.add_window(|_, _| CompactSectionFixture {
        masks: masks.clone(),
    });
    let mut visual = VisualTestContext::from_window(*handle, cx);
    visual.run_until_parked();
    let scale = visual.update(|window, _| window.scale_factor());
    for (section, mask_name, inset, radius) in [
        (
            "vertical-section",
            "vertical",
            DEVICE_SECTION_INSET,
            DEVICE_SECTION_RADIUS,
        ),
        ("dense-section", "dense", 1.0, 1.0),
    ] {
        let bounds = visual.debug_bounds(section).expect("section");
        let mask = masks
            .borrow()
            .iter()
            .find(|(name, _)| *name == mask_name)
            .unwrap()
            .1;
        assert_eq!(bounds.size.width, px(64.0));
        assert_eq!(mask.origin.x, bounds.origin.x, "no horizontal width tax");
        assert_eq!(mask.size.width, bounds.size.width);
        assert_eq!(mask.origin.y, bounds.origin.y + px(inset));
        assert_eq!(mask.size.height, bounds.size.height - px(inset * 2.0));
        assert_eq!(
            visual
                .rendered_surface_geometry(section)
                .unwrap()
                .0
                .top_left,
            px(radius).scale(scale)
        );
        assert_eq!(
            visual.rendered_background(section),
            Some(gpui::Background::from(rgb(STYLE.rail)))
        );
    }
    for name in [
        "full-width-control",
        "dense-first",
        "dense-middle",
        "dense-last",
    ] {
        assert_eq!(
            visual.debug_bounds(name).unwrap().size,
            size(px(64.0), px(52.0))
        );
    }
    let dense = visual.debug_bounds("dense-section").unwrap();
    assert_eq!(
        visual.debug_bounds("dense-last").unwrap().bottom_left().y,
        dense.bottom_left().y - px(1.0)
    );
    assert_eq!(
        visual.debug_bounds("short-control").unwrap().size.height,
        px(14.0)
    );
    let action = visual.debug_bounds("shared-action").unwrap();
    assert_eq!(
        action.size,
        size(px(DEVICE_HEADER_ACTION_SIZE), px(DEVICE_HEADER_ACTION_SIZE))
    );
    assert_eq!(
        visual.rendered_background("shared-action"),
        Some(gpui::Background::from(rgb(0x123456)))
    );
    visual.simulate_mouse_move(action.center(), None, Modifiers::default());
    visual.run_until_parked();
    assert_eq!(
        visual.rendered_background("shared-action"),
        Some(gpui::Background::from(rgb(0xabcdef)))
    );
}

struct SharedActionFixture {
    clicks: Rc<Cell<usize>>,
}
impl Render for SharedActionFixture {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let style = ActionStyle {
            background: 0x123456,
            text: 0xffffff,
            hover_background: 0xabcdef,
            hover_text: Some(0),
        };
        let clicks = self.clicks.clone();
        div()
            .flex()
            .gap(px(4.0))
            .child(
                device_action("×".into(), style)
                    .id("device-action")
                    .debug_selector(|| "device-action".into()),
            )
            .child(
                action("play".into(), style)
                    .id("workspace-action")
                    .debug_selector(|| "workspace-action".into())
                    .w(px(25.0))
                    .h(px(22.0))
                    .on_click(move |_, _, _| clicks.set(clicks.get() + 1)),
            )
            .child(
                toggle_action("solo".into(), style.with_state_colors(0x123456, 0xffffff))
                    .id("state-action")
                    .debug_selector(|| "state-action".into())
                    .size(px(18.0)),
            )
    }
}

#[gpui::test]
fn shared_actions_preserve_owner_sizes_clicks_and_common_hover(cx: &mut TestAppContext) {
    let clicks = Rc::new(Cell::new(0));
    let handle = cx.add_window(|_, _| SharedActionFixture {
        clicks: clicks.clone(),
    });
    let mut visual = VisualTestContext::from_window(*handle, cx);
    // The simulated pointer starts at the origin, inside the first control.
    visual.simulate_mouse_move(point(px(200.0), px(200.0)), None, Modifiers::default());
    visual.run_until_parked();
    let scale = visual.update(|window, _| window.scale_factor());
    for (name, width, height) in [
        (
            "device-action",
            DEVICE_HEADER_ACTION_SIZE,
            DEVICE_HEADER_ACTION_SIZE,
        ),
        ("workspace-action", 25.0, 22.0),
        ("state-action", 18.0, 18.0),
    ] {
        let bounds = visual.debug_bounds(name).unwrap();
        assert_eq!(bounds.size, size(px(width), px(height)));
        assert_eq!(
            visual.rendered_surface_geometry(name).unwrap().0.top_left,
            px(if name == "state-action" {
                crate::TOGGLE_CORNER_RADIUS
            } else {
                width.min(height) / 2.0
            })
            .scale(scale)
        );
        assert_eq!(
            visual.rendered_background(name),
            Some(gpui::Background::from(rgb(0x123456)))
        );
        visual.simulate_mouse_move(bounds.center(), None, Modifiers::default());
        visual.run_until_parked();
        assert_ne!(
            visual.rendered_background(name),
            Some(gpui::Background::from(rgb(0x123456))),
            "{name} has visible hover feedback"
        );
        visual.simulate_click(bounds.center(), Modifiers::default());
        visual.simulate_mouse_move(point(px(200.0), px(200.0)), None, Modifiers::default());
        visual.run_until_parked();
    }
    assert_eq!(
        clicks.get(),
        1,
        "only the workspace owner installed a click handler"
    );
}

struct HousingFixture {
    reverse: bool,
    actions: usize,
}
impl Render for HousingFixture {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let order = if self.reverse { ["b", "a"] } else { ["a", "b"] };
        div().flex().children(order.map(|id| {
            device_frame(
                "device",
                STYLE,
                div(),
                crate::device_action(
                    "+".into(),
                    crate::ActionStyle {
                        background: STYLE.raised,
                        text: STYLE.text,
                        hover_background: STYLE.hover,
                        hover_text: Some(STYLE.hover_text),
                    },
                )
                .id("caller-action")
                .debug_selector(move || format!("action-{id}"))
                .on_click(cx.listener(|view, _, _, cx| {
                    view.actions += 1;
                    cx.notify();
                })),
                device_body(STYLE).h(px(120.0)).child("device controls"),
            )
            .id(id)
            .debug_selector(move || id.into())
            .w(px(142.0))
            .h(px(150.0))
            .flex_shrink_0()
        }))
    }
}
#[gpui::test]
fn shared_housing_panels_toggle_independently_and_follow_device_identity(cx: &mut TestAppContext) {
    let handle = cx.add_window(|_, _| HousingFixture {
        reverse: false,
        actions: 0,
    });
    let view = handle.root(cx).unwrap();
    let mut visual = VisualTestContext::from_window(*handle, cx);
    visual.simulate_resize(size(px(400.0), px(200.0)));
    visual.run_until_parked();
    assert!(visual.debug_bounds("device-macro-panel").is_none());
    assert!(visual.debug_bounds("device-modulator-panel").is_none());
    let a = visual.debug_bounds("a").unwrap();
    let b = visual.debug_bounds("b").unwrap();
    let macro_b = visual.debug_bounds("device-macro-toggle").unwrap();
    let mod_b = visual.debug_bounds("device-modulator-toggle").unwrap();
    let macro_a = macro_b.center() + point(a.left() - b.left(), px(0.0));
    visual.simulate_click(macro_a, Modifiers::default());
    visual.run_until_parked();
    let panel = visual
        .debug_bounds("device-macro-panel")
        .expect("first housing opened");
    assert!(a.contains(&panel.center()));
    assert!(visual.debug_bounds("device-modulator-panel").is_none());
    visual.simulate_keystrokes("tab enter");
    visual.run_until_parked();
    assert!(
        a.contains(
            &visual
                .debug_bounds("device-modulator-panel")
                .unwrap()
                .center()
        )
    );
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    assert!(visual.debug_bounds("device-modulator-panel").is_none());
    visual.simulate_click(mod_b.center(), Modifiers::default());
    visual.run_until_parked();
    assert!(
        b.contains(
            &visual
                .debug_bounds("device-modulator-panel")
                .unwrap()
                .center()
        )
    );
    assert!(a.contains(&visual.debug_bounds("device-macro-panel").unwrap().center()));
    // The second housing can show both panels without changing the first.
    visual.simulate_click(macro_b.center(), Modifiers::default());
    visual.run_until_parked();
    assert!(b.contains(&visual.debug_bounds("device-macro-panel").unwrap().center()));
    visual.simulate_keystrokes("space");
    visual.run_until_parked();
    assert!(a.contains(&visual.debug_bounds("device-macro-panel").unwrap().center()));
    let caller = visual.debug_bounds("action-b").unwrap();
    visual.simulate_click(caller.center(), Modifiers::default());
    visual.run_until_parked();
    view.read_with(cx, |view, _| assert_eq!(view.actions, 1));
    view.update(cx, |view, cx| {
        view.reverse = true;
        cx.notify();
    });
    visual.run_until_parked();
    let a = visual.debug_bounds("a").unwrap();
    let b = visual.debug_bounds("b").unwrap();
    assert!(a.contains(&visual.debug_bounds("device-macro-panel").unwrap().center()));
    assert!(
        b.contains(
            &visual
                .debug_bounds("device-modulator-panel")
                .unwrap()
                .center()
        )
    );
    // Close and reopen the same panel after reordering.
    let macro_a = visual.debug_bounds("device-macro-toggle").unwrap();
    visual.simulate_click(macro_a.center(), Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("device-macro-panel").is_none());
    visual.simulate_click(macro_a.center(), Modifiers::default());
    visual.run_until_parked();
    assert!(a.contains(&visual.debug_bounds("device-macro-panel").unwrap().center()));
}
