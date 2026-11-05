use super::*;
use gpui::{Empty, Render, TestAppContext, VisualTestContext};
use std::{cell::RefCell, rc::Rc};

struct Root(Entity<BarEditor>);
impl Render for Root {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .child(div().w(px(320.0)).h(px(80.0)).child(self.0.clone()))
    }
}

#[gpui::test]
fn right_stroke_reports_all_crossed_columns_and_its_own_terminal(cx: &mut TestAppContext) {
    let bootstrap = cx.add_window(|_, _| Empty);
    let handle = *bootstrap;
    let bars = cx
        .update_window(handle, |_, window, cx| {
            let bars = cx.new(|cx| BarEditor::new(cx, "bars", crate::tests::STYLE));
            bars.update(cx, |bars, cx| bars.set_values([0.8; 64], 8, cx).unwrap());
            window.replace_root(cx, |_, _| Root(bars.clone()));
            bars
        })
        .unwrap();
    let events = Rc::new(RefCell::new(Vec::new()));
    let captured = events.clone();
    let _subscription = cx
        .update(|app| app.subscribe(&bars, move |_, event, _| captured.borrow_mut().push(*event)));
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let bounds = visual.debug_bounds("bars").unwrap();
    let at = |column: f32, value: f32| {
        point(
            bounds.left() + bounds.size.width * (column + 0.5) / 8.0,
            bounds.bottom() - bounds.size.height * value,
        )
    };
    let shift = Modifiers {
        shift: true,
        ..Default::default()
    };
    visual.simulate_mouse_down(at(2.0, 0.8), MouseButton::Right, shift);
    assert_eq!(events.borrow()[0].columns, [2, 2]);
    visual.simulate_mouse_move(at(7.0, 0.9), Some(MouseButton::Right), shift);
    visual.simulate_mouse_up(at(7.0, 0.9), MouseButton::Left, shift);
    assert_eq!(
        events.borrow().len(),
        2,
        "unrelated release cannot end a right stroke"
    );
    visual.simulate_mouse_up(at(-2.0, 0.3), MouseButton::Right, shift);
    let reported = events.borrow();
    assert_eq!(
        reported.iter().map(|e| e.phase).collect::<Vec<_>>(),
        [
            BarEditorPhase::Begin,
            BarEditorPhase::Change,
            BarEditorPhase::Commit
        ]
    );
    assert_eq!(reported[1].columns, [2, 7]);
    assert_eq!(reported[2].columns, [0, 7]);
    assert!(
        reported
            .iter()
            .all(|e| matches!(e.mode, BarEditorMode::Clear) && e.position[1] == 0.0)
    );
    drop(reported);
    events.borrow_mut().clear();

    // Paint still supplies its pointer value; a lost button cancels clearing.
    visual.simulate_mouse_down(at(1.0, 0.75), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(at(1.0, 0.75), MouseButton::Left, Modifiers::default());
    assert!(matches!(events.borrow()[0].mode, BarEditorMode::Paint));
    assert_eq!(events.borrow()[0].position[1], 0.75);
    visual.simulate_mouse_down(at(3.0, 0.2), MouseButton::Right, Modifiers::default());
    visual.simulate_mouse_move(at(4.0, 0.2), None, Modifiers::default());
    assert_eq!(
        events.borrow().last().unwrap().phase,
        BarEditorPhase::Cancel
    );
    visual.simulate_mouse_down(at(3.0, 0.2), MouseButton::Right, Modifiers::default());
    visual.simulate_keystrokes("escape");
    assert_eq!(
        events.borrow().last().unwrap().phase,
        BarEditorPhase::Cancel
    );
    visual.simulate_mouse_down(at(3.0, 0.2), MouseButton::Right, Modifiers::default());
    bars.update(cx, |bars, cx| bars.set_enabled(false, cx));
    assert_eq!(
        events.borrow().last().unwrap().phase,
        BarEditorPhase::Cancel
    );
    let count = events.borrow().len();
    visual.run_until_parked();
    visual.simulate_mouse_up(at(7.0, 0.0), MouseButton::Right, Modifiers::default());
    for button in [MouseButton::Left, MouseButton::Right] {
        visual.simulate_mouse_down(at(0.0, 0.5), button, Modifiers::default());
        visual.simulate_mouse_up(at(0.0, 0.5), button, Modifiers::default());
    }
    visual.simulate_keystrokes("up");
    assert_eq!(events.borrow().len(), count);
}
