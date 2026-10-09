//! Reusable compact popup presentation; owners dispatch typed commands.
use crate::theme;
use gpui::{
    BoxShadow, Context, Div, FocusHandle, MouseButton, Pixels, Point, Window, div, hsla, point,
    prelude::*, px, rgb,
};

const WIDTH: f32 = 210.0;
const ROW_HEIGHT: f32 = 24.0;
const EDGE: f32 = 4.0;

pub(crate) struct MenuItem<A> {
    pub label: &'static str,
    pub action: A,
}

pub(crate) struct PopupMenu<A> {
    pub position: Point<Pixels>,
    pub items: Vec<MenuItem<A>>,
    pub selected: usize,
    pub focus: FocusHandle,
    pub previous_focus: Option<FocusHandle>,
}

fn clamped_position(
    position: Point<Pixels>,
    window_size: gpui::Size<Pixels>,
    height: f32,
) -> Point<Pixels> {
    point(
        px(f32::from(position.x).clamp(
            EDGE,
            (f32::from(window_size.width) - WIDTH - EDGE).max(EDGE),
        )),
        px(f32::from(position.y).clamp(
            EDGE,
            (f32::from(window_size.height) - height - EDGE).max(EDGE),
        )),
    )
}

type Select<T, A> = fn(&mut T, A, &mut Window, &mut Context<T>);
type Dismiss<T> = fn(&mut T, &mut Window, &mut Context<T>);
type Navigate<T> = fn(&mut T, bool, &mut Context<T>);

pub(crate) fn popup<T: 'static, A: Clone + 'static>(
    menu: &PopupMenu<A>,
    window: &Window,
    cx: &mut Context<T>,
    select: Select<T, A>,
    dismiss: Dismiss<T>,
    navigate: Navigate<T>,
) -> Div {
    let height = menu.items.len() as f32 * ROW_HEIGHT + 6.0;
    let position = clamped_position(menu.position, window.bounds().size, height);
    let selected_action = menu
        .items
        .get(menu.selected)
        .map(|item| item.action.clone());
    let mut panel = div()
        .id("context-menu-panel")
        .debug_selector(|| "context-menu-panel".into())
        .absolute()
        .left(position.x)
        .top(position.y)
        .w(px(WIDTH))
        .py(px(2.0))
        .border_1()
        .border_color(rgb(theme::color(theme::PaletteRole::Muted)))
        .bg(rgb(theme::color(theme::PaletteRole::Surface)))
        .text_xs()
        .shadow(vec![BoxShadow {
            color: hsla(0.0, 0.0, 0.0, 0.5),
            blur_radius: px(12.0),
            spread_radius: px(0.0),
            offset: point(px(0.0), px(4.0)),
        }])
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation());
    for (index, item) in menu.items.iter().enumerate() {
        let action = item.action.clone();
        panel = panel.children([div()
            .id(("context-menu-item", index))
            .debug_selector(|| "context-menu-item".into())
            .h(px(ROW_HEIGHT))
            .px(px(10.0))
            .flex()
            .items_center()
            .cursor_pointer()
            .bg(rgb(if menu.selected == index {
                theme::color(theme::PaletteRole::Hover)
            } else {
                theme::color(theme::PaletteRole::Surface)
            }))
            .hover(|style| style.bg(rgb(theme::color(theme::PaletteRole::PrimaryDark))))
            .child(theme::text(item.label))
            .on_click(
                cx.listener(move |owner, _, window, cx| select(owner, action.clone(), window, cx)),
            )]);
    }
    div()
        .absolute()
        .inset_0()
        .occlude()
        .track_focus(&menu.focus)
        .on_key_down(
            cx.listener(move |owner, event: &gpui::KeyDownEvent, window, cx| {
                cx.stop_propagation();
                match event.keystroke.key.as_str() {
                    "escape" => dismiss(owner, window, cx),
                    "enter" => {
                        if let Some(action) = &selected_action {
                            select(owner, action.clone(), window, cx);
                        }
                    }
                    "up" => navigate(owner, false, cx),
                    "down" => navigate(owner, true, cx),
                    _ => {}
                }
            }),
        )
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |owner, _, window, cx| {
                cx.stop_propagation();
                dismiss(owner, window, cx);
            }),
        )
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |owner, _, window, cx| {
                cx.stop_propagation();
                dismiss(owner, window, cx);
            }),
        )
        .on_mouse_down(
            MouseButton::Middle,
            cx.listener(move |owner, _, window, cx| {
                cx.stop_propagation();
                dismiss(owner, window, cx);
            }),
        )
        .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
        .children([panel])
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn popup_stays_inside_window_edges() {
        assert_eq!(
            clamped_position(
                point(px(990.0), px(790.0)),
                gpui::size(px(1000.0), px(800.0)),
                30.0
            ),
            point(px(786.0), px(766.0))
        );
        assert_eq!(
            clamped_position(
                point(px(-10.0), px(-20.0)),
                gpui::size(px(1000.0), px(800.0)),
                30.0
            ),
            point(px(4.0), px(4.0))
        );
    }
}
