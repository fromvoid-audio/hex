//! Completed-frame option feedback, sizing and input regression coverage.
use super::*;
use gpui::{AppContext, Empty, Entity, Render, TestAppContext, VisualTestContext, size};
use std::{cell::RefCell, rc::Rc};

const STYLE: ControlStyle = ControlStyle {
    background: 0x202020,
    surface: 0x303030,
    raised: 0x383838,
    rail: 0x282828,
    border: 0x505050,
    text: 0xe0e0e0,
    muted: 0xaaaaaa,
    accent: 0x55ddee,
    accent_text: 0x202020,
    numeric_fill: 0x31515a,
    hover_text: 0xe0e0e0,
    focus_marker: 0xaaaaaa,
    dial_rail: 0x000000,
    hover: 0x404040,
    selection: 0x3350a0,
    invalid: 0xff554c,
    font_size: 10.0,
    text_case: crate::TextCase::Preserve,
};
gpui::actions!(selector_test_host, [HostShortcut]);
struct Root {
    selector: Entity<Selector>,
    width: f32,
    top: f32,
    height: f32,
    host_shortcuts: usize,
}
impl Render for Root {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .key_context("selector_test_host")
            .on_action(cx.listener(|root, _: &HostShortcut, _, cx| {
                root.host_shortcuts += 1;
                cx.stop_propagation();
            }))
            .debug_selector(|| "selector-slot".into())
            .w(px(self.width))
            .pt(px(self.top))
            .h(px(self.top + self.height))
            .overflow_hidden()
            .child(self.selector.clone())
    }
}
fn fixture(
    cx: &mut TestAppContext,
    appearance: SelectorAppearance,
) -> (gpui::AnyWindowHandle, Entity<Selector>, Entity<Root>) {
    cx.update(bind_selector_keys);
    let handle = *cx.add_window(|_, _| Empty);
    let (selector, root) = cx
        .update_window(handle, |_, window, cx| {
            let selector = cx.new(|cx| {
                Selector::from_config(
                    cx,
                    SelectorConfig::new(
                        "choice",
                        ["a", "medium", "a_very_long_choice_name"],
                        0,
                        STYLE,
                    )
                    .unwrap()
                    .with_appearance(appearance),
                )
            });
            let root = window.replace_root(cx, |_, _| Root {
                selector: selector.clone(),
                width: 180.0,
                top: 0.0,
                height: 24.0,
                host_shortcuts: 0,
            });
            (selector, root)
        })
        .unwrap();
    (handle, selector, root)
}
fn background(visual: &mut VisualTestContext, name: &'static str, color: u32) {
    assert_eq!(
        visual.rendered_background(name),
        Some(gpui::Background::from(rgb(color)))
    );
}

#[gpui::test]
fn staged_popup_navigation_confirms_once_and_cancellation_preserves_value(cx: &mut TestAppContext) {
    let (handle, selector, _) = fixture(cx, SelectorAppearance::CompactSelector);
    selector.update(cx, |s, cx| s.set_confirm_on_enter(true, cx));
    let changed = Rc::new(RefCell::new(Vec::new()));
    let captured = changed.clone();
    let _subscription = cx.update(|app| {
        app.subscribe(&selector, move |_, event, _| {
            captured.borrow_mut().push(event.index);
        })
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(400.0), px(240.0)));
    visual.run_until_parked();
    let field = visual.debug_bounds("choice").unwrap().center();
    visual.simulate_click(field, gpui::Modifiers::default());
    visual.simulate_keystrokes("down");
    visual.run_until_parked();
    assert_eq!(selector.read_with(cx, |s, _| s.selected()), 0);
    assert!(changed.borrow().is_empty());
    background(&mut visual, "choice-option-1", STYLE.hover);
    background(&mut visual, "choice-option-marker-0", STYLE.accent);
    assert!(visual.debug_bounds("choice-option-marker-1").is_none());
    // Routine owner repaints must preserve the candidate.
    selector.update(cx, |s, cx| {
        s.set_selected(0, cx).unwrap();
        s.set_confirm_on_enter(true, cx);
    });
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    assert_eq!(*changed.borrow(), [1]);
    assert!(!selector.read_with(cx, |s, _| s.is_open()));
    for outside in [false, true] {
        visual.simulate_click(field, gpui::Modifiers::default());
        visual.simulate_keystrokes("down");
        if outside {
            visual.simulate_click(point(px(300.0), px(200.0)), gpui::Modifiers::default());
        } else {
            visual.simulate_keystrokes("escape");
        }
        visual.run_until_parked();
        assert!(!selector.read_with(cx, |s, _| s.is_open()));
        assert_eq!(selector.read_with(cx, |s, _| s.selected()), 1);
        assert_eq!(*changed.borrow(), [1]);
    }
}

#[gpui::test]
fn staged_popup_scrolls_candidates_and_reconciles_external_values(cx: &mut TestAppContext) {
    let (handle, selector, _) = fixture(cx, SelectorAppearance::CompactSelector);
    selector.update(cx, |s, cx| {
        s.choices = (0..40).map(|i| i.to_string().into()).collect();
        s.set_selected(39, cx).unwrap();
        s.set_confirm_on_enter(true, cx);
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(400.0), px(120.0)));
    visual.run_until_parked();
    let field = visual.debug_bounds("choice").unwrap().center();
    visual.simulate_click(field, gpui::Modifiers::default());
    visual.run_until_parked();
    let popup = visual.debug_bounds("choice-popup").unwrap();
    assert!(
        popup.contains(&visual.debug_bounds("choice-option-39").unwrap().center()),
        "popup={popup:?} option={:?} scroll={:?}",
        visual.debug_bounds("choice-option-39"),
        selector.read_with(cx, |s, _| s.popup_scroll.offset())
    );
    visual.simulate_keystrokes("down");
    visual.run_until_parked();
    assert!(popup.contains(&visual.debug_bounds("choice-option-0").unwrap().center()));
    assert_eq!(selector.read_with(cx, |s, _| s.selected()), 39);
    selector.update(cx, |s, cx| s.set_selected(20, cx).unwrap());
    visual.run_until_parked();
    assert!(popup.contains(&visual.debug_bounds("choice-option-20").unwrap().center()));
    visual.simulate_keystrokes("enter");
    assert_eq!(selector.read_with(cx, |s, _| s.selected()), 20);
    visual.simulate_keystrokes("enter");
    visual.simulate_keystrokes("down");
    selector.update(cx, |s, cx| s.set_enabled(false, cx));
    visual.run_until_parked();
    assert!(!selector.read_with(cx, |s, _| s.is_open()));
    assert_eq!(selector.read_with(cx, |s, _| s.selected()), 20);
}

#[gpui::test]
fn focused_popup_keys_precede_host_actions_and_closed_escape_still_reaches_host(
    cx: &mut TestAppContext,
) {
    cx.update(|cx| {
        cx.bind_keys(
            ["enter", "space", "escape", "up", "down"]
                .map(|key| gpui::KeyBinding::new(key, HostShortcut, Some("selector_test_host"))),
        )
    });
    let (handle, selector, root) = fixture(cx, SelectorAppearance::CompactSelector);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let field = visual.debug_bounds("choice").unwrap().center();
    visual.simulate_click(field, gpui::Modifiers::default());
    visual.run_until_parked();
    assert!(selector.read_with(cx, |selector, _| selector.is_open()));
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    assert!(!selector.read_with(cx, |selector, _| selector.is_open()));
    assert_eq!(root.read_with(cx, |root, _| root.host_shortcuts), 0);
    visual.simulate_keystrokes("escape");
    assert_eq!(root.read_with(cx, |root, _| root.host_shortcuts), 1);
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    assert!(selector.read_with(cx, |selector, _| selector.is_open()));
    visual.simulate_keystrokes("down");
    visual.run_until_parked();
    assert_eq!(selector.read_with(cx, |selector, _| selector.selected()), 1);
    assert!(!selector.read_with(cx, |selector, _| selector.is_open()));
    assert_eq!(root.read_with(cx, |root, _| root.host_shortcuts), 1);
    visual.simulate_keystrokes("space");
    visual.run_until_parked();
    assert!(selector.read_with(cx, |selector, _| selector.is_open()));
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    selector.update(cx, |selector, cx| selector.set_enabled(false, cx));
    visual.simulate_keystrokes("enter");
    assert_eq!(
        root.read_with(cx, |root, _| root.host_shortcuts),
        2,
        "unavailable selector leaves host keys alone"
    );
}

#[gpui::test]
fn popup_options_hover_without_choosing_and_keep_aligned_sizes(cx: &mut TestAppContext) {
    for appearance in [
        SelectorAppearance::Standard,
        SelectorAppearance::CompactSelector,
    ] {
        let (handle, selector, root) = fixture(cx, appearance);
        let changed = Rc::new(RefCell::new(Vec::new()));
        let captured = changed.clone();
        let _subscription = cx.update(|app| {
            app.subscribe(&selector, move |_, event, _| {
                captured.borrow_mut().push(event.index)
            })
        });
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.simulate_resize(size(px(400.0), px(240.0)));
        for width in [90.0, 210.0] {
            root.update(cx, |root, cx| {
                root.width = width;
                cx.notify();
            });
            visual.run_until_parked();
            let field = visual.debug_bounds("choice").unwrap();
            visual.simulate_click(field.center(), gpui::Modifiers::default());
            visual.run_until_parked();
            let rows = ["choice-option-0", "choice-option-1", "choice-option-2"]
                .map(|name| visual.debug_bounds(name).unwrap());
            let slot = visual.debug_bounds("selector-slot").unwrap();
            assert!(
                rows[1].center().y > slot.bottom(),
                "hover is outside the clipped owner"
            );
            for row in rows {
                assert_eq!(row.size, rows[0].size);
            }
            visual.simulate_mouse_move(rows[1].center(), None, gpui::Modifiers::default());
            visual.run_until_parked();
            background(&mut visual, "choice-option-1", STYLE.hover);
            assert_eq!(selector.read_with(cx, |s, _| s.selected()), 0);
            assert!(changed.borrow().is_empty(), "hover cannot edit the value");
            assert_eq!(visual.debug_bounds("choice").unwrap(), field);
            background(&mut visual, "choice-option-marker-0", STYLE.accent);
            assert!(visual.debug_bounds("choice-option-marker-1").is_none());
            visual.simulate_keystrokes("escape");
            visual.run_until_parked();
        }
        selector.update(cx, |s, cx| s.set_enabled(false, cx));
        let field = visual.debug_bounds("choice").unwrap();
        visual.simulate_click(field.center(), gpui::Modifiers::default());
        visual.run_until_parked();
        assert!(visual.debug_bounds("choice-popup").is_none());
        assert!(changed.borrow().is_empty());
    }
}

#[gpui::test]
fn compact_segments_focus_only_chosen_option_and_share_widths(cx: &mut TestAppContext) {
    let (handle, selector, _) = fixture(cx, SelectorAppearance::CompactSegments);
    let changed = Rc::new(RefCell::new(Vec::new()));
    let captured = changed.clone();
    let _subscription = cx.update(|app| {
        app.subscribe(&selector, move |_, event, _| {
            captured.borrow_mut().push(event.index)
        })
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(400.0), px(240.0)));
    visual.run_until_parked();
    let names = ["choice-option-0", "choice-option-1", "choice-option-2"];
    let second = visual.debug_bounds(names[1]).unwrap();
    visual.simulate_click(second.center(), gpui::Modifiers::default());
    visual.run_until_parked();
    assert_eq!(selector.read_with(cx, |s, _| s.selected()), 1);
    for name in [names[0], names[2]] {
        assert_eq!(
            visual.rendered_border(name).unwrap().0,
            gpui::Hsla::from(rgb(STYLE.border)),
            "unselected borders must not light up with group focus"
        );
    }
    let rows = names.map(|name| visual.debug_bounds(name).unwrap());
    for row in rows {
        assert!(
            (row.size.width - rows[0].size.width).abs() <= px(1.0),
            "unequal labels still have equal option widths"
        );
        assert_eq!(row.size.height, rows[0].size.height);
    }
    visual.simulate_mouse_move(rows[2].center(), None, gpui::Modifiers::default());
    visual.run_until_parked();
    background(&mut visual, names[2], STYLE.hover);
    background(&mut visual, names[1], STYLE.accent);
    assert_eq!(*changed.borrow(), [1]);
    visual.simulate_keystrokes("right");
    visual.run_until_parked();
    assert_eq!(selector.read_with(cx, |s, _| s.selected()), 2);
    assert_eq!(*changed.borrow(), [1, 2]);
    selector.update(cx, |s, cx| s.set_enabled(false, cx));
    visual.simulate_mouse_move(rows[0].center(), None, gpui::Modifiers::default());
    visual.simulate_click(rows[0].center(), gpui::Modifiers::default());
    visual.simulate_keystrokes("right");
    visual.run_until_parked();
    background(&mut visual, names[0], STYLE.surface);
    assert_eq!(*changed.borrow(), [1, 2]);
}

#[gpui::test]
fn compact_popup_and_field_form_one_aligned_rectangle(cx: &mut TestAppContext) {
    let (handle, selector, root) = fixture(cx, SelectorAppearance::CompactSelector);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(400.0), px(240.0)));
    root.update(cx, |root, cx| {
        root.width = 134.0;
        cx.notify();
    });
    visual.run_until_parked();
    let field = visual.debug_bounds("choice").unwrap();
    visual.simulate_click(field.center(), gpui::Modifiers::default());
    visual.run_until_parked();
    let popup = visual.debug_bounds("choice-popup").unwrap();
    assert_eq!(
        popup.left(),
        field.left(),
        "left edges: {field:?} {popup:?}"
    );
    assert_eq!(
        popup.right(),
        field.right(),
        "right edges: {field:?} {popup:?}"
    );
    assert_eq!(
        popup.top(),
        field.bottom() - px(1.0),
        "one shared border: {field:?} {popup:?}"
    );
    let scale = visual.update(|window, _| window.scale_factor());
    let corners = visual.rendered_surface_geometry("choice-popup").unwrap().0;
    assert_eq!(corners.top_left, px(0.0).scale(scale));
    assert_eq!(corners.bottom_right, px(0.0).scale(scale));
    for name in ["choice-option-0", "choice-option-1", "choice-option-2"] {
        assert_eq!(
            visual.debug_bounds(name).unwrap().size.height,
            field.size.height
        );
    }
    let last = visual.debug_bounds("choice-option-2").unwrap();
    visual.simulate_click(last.center(), gpui::Modifiers::default());
    visual.run_until_parked();
    selector.read_with(cx, |selector, _| {
        assert_eq!(selector.selected(), 2);
        assert!(!selector.is_open());
    });
    root.update(cx, |root, cx| {
        root.top = 220.0;
        cx.notify();
    });
    visual.run_until_parked();
    let field = visual.debug_bounds("choice").unwrap();
    visual.simulate_click(field.center(), gpui::Modifiers::default());
    visual.run_until_parked();
    let popup = visual.debug_bounds("choice-popup").unwrap();
    assert_eq!(
        popup.bottom(),
        field.top() + px(1.0),
        "open above with one shared border"
    );
    assert_eq!(popup.left(), field.left());
    assert_eq!(popup.right(), field.right());
    assert!(popup.top() >= px(0.0));
    let first = visual.debug_bounds("choice-option-0").unwrap();
    visual.simulate_click(first.center(), gpui::Modifiers::default());
    visual.run_until_parked();
    selector.read_with(cx, |selector, _| assert_eq!(selector.selected(), 0));
}

#[gpui::test]
fn segmented_grid_rows_are_bounded_and_vertical_keys_follow_columns(cx: &mut TestAppContext) {
    for columns in [0, 129, usize::MAX] {
        assert!(
            SelectorConfig::new("grid", ["a"], 0, STYLE)
                .unwrap()
                .with_segment_columns(columns)
                .is_err()
        );
    }
    let (handle, selector, root) = fixture(cx, SelectorAppearance::CompactSegments);
    let grid = cx.new(|cx| {
        Selector::from_config(
            cx,
            SelectorConfig::new("grid", ["1", "2", "3", "4", "5", "6", "8", "16"], 0, STYLE)
                .unwrap()
                .with_segment_columns(2)
                .unwrap(),
        )
    });
    root.update(cx, |root, cx| {
        root.selector = grid.clone();
        root.width = 64.0;
        root.height = 70.0;
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(200.0), px(160.0)));
    visual.run_until_parked();
    let first = visual.debug_bounds("grid-option-0").unwrap();
    let second = visual.debug_bounds("grid-option-1").unwrap();
    let third = visual.debug_bounds("grid-option-2").unwrap();
    let last = visual.debug_bounds("grid-option-7").unwrap();
    assert_eq!(first.top(), second.top());
    assert!(first.right() <= second.left());
    assert!(first.bottom() <= third.top());
    assert!(last.bottom() <= visual.debug_bounds("selector-slot").unwrap().bottom());
    visual.simulate_click(first.center(), gpui::Modifiers::default());
    visual.simulate_keystrokes("down");
    assert_eq!(grid.read_with(cx, |g, _| g.selected), 2);
    visual.simulate_keystrokes("right");
    assert_eq!(grid.read_with(cx, |g, _| g.selected), 3);
    visual.simulate_keystrokes("up");
    assert_eq!(grid.read_with(cx, |g, _| g.selected), 1);
    assert_eq!(selector.read_with(cx, |g, _| g.selected), 0);
}
