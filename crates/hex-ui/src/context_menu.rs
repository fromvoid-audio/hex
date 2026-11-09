//! Reusable compact popup presentation; owners dispatch typed commands.
use crate::theme;
use gpui::{
    BoxShadow, Context, Div, FocusHandle, MouseButton, Pixels, Point, Window, div, hsla, point,
    prelude::*, px, rgb,
};

const WIDTH: f32 = 210.0;
const ROW_HEIGHT: f32 = 24.0;
const EDGE: f32 = 4.0;
const ICON_COLUMNS: usize = 4;
const ICON_CELL: f32 = 28.0;
const ICON_WIDTH: f32 = ICON_COLUMNS as f32 * ICON_CELL + 8.0;

pub(crate) struct MenuItem<A> {
    pub label: &'static str,
    pub swatch: Option<u32>,
    pub checked: bool,
    pub action: A,
}

pub(crate) struct PopupMenu<A> {
    pub scroll: gpui::ScrollHandle,
    pub position: Point<Pixels>,
    // Item zero and any items after the icon catalog are full-width actions.
    pub icon_grid: Option<Vec<&'static str>>,
    pub items: Vec<MenuItem<A>>,
    pub selected: usize,
    pub focus: FocusHandle,
    pub previous_focus: Option<FocusHandle>,
}

fn clamped_position(
    position: Point<Pixels>,
    window_size: gpui::Size<Pixels>,
    height: f32,
    width: f32,
) -> Point<Pixels> {
    point(
        px(f32::from(position.x).clamp(
            EDGE,
            (f32::from(window_size.width) - width - EDGE).max(EDGE),
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
    popup_with_labels(menu, window, cx, select, dismiss, navigate, |item| {
        item.label.into()
    })
}

/// Owners may supply real instance/parameter names without leaking static strings.
pub(crate) fn popup_with_labels<T: 'static, A: Clone + 'static>(
    menu: &PopupMenu<A>,
    window: &Window,
    cx: &mut Context<T>,
    select: Select<T, A>,
    dismiss: Dismiss<T>,
    navigate: Navigate<T>,
    label_for: fn(&MenuItem<A>) -> gpui::SharedString,
) -> Div {
    let grid = menu.icon_grid.as_ref();
    let (width, height) = if grid.is_some() {
        (
            ICON_WIDTH,
            ROW_HEIGHT * menu.items.len().saturating_sub(grid.map_or(0, Vec::len)) as f32
                + grid.map_or(0, Vec::len).div_ceil(ICON_COLUMNS) as f32 * ICON_CELL
                + 13.0,
        )
    } else {
        (WIDTH, menu.items.len() as f32 * ROW_HEIGHT + 6.0)
    };
    let height = height.min((f32::from(window.bounds().size.height) - 2.0 * EDGE).max(ROW_HEIGHT));
    let position = clamped_position(menu.position, window.bounds().size, height, width);
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
        .w(px(width))
        .flex()
        .flex_col()
        .max_h(px(height))
        .overflow_y_scroll()
        .track_scroll(&menu.scroll)
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
    let is_icon_menu = grid.is_some();
    let mut cells = Vec::new();
    let mut trailing = Vec::new();
    for (index, item) in menu.items.iter().enumerate() {
        let action = item.action.clone();
        let label = label_for(item);
        let tooltip_label = label.clone();
        let swatch_label = label.clone();
        let glyph = grid
            .and_then(|icons| index.checked_sub(1).and_then(|index| icons.get(index)))
            .copied();
        let row = hex_widgets::action_surface(hex_widgets::ActionStyle {
            background: theme::color(theme::PaletteRole::Surface),
            text: theme::color(theme::PaletteRole::Text),
            hover_background: theme::color(theme::PaletteRole::PrimaryDark),
            hover_text: None,
        })
        .justify_start()
        .id(("context-menu-item", index))
        .debug_selector(move || {
            if is_icon_menu {
                format!("context-menu-choice-{index}")
            } else {
                "context-menu-item".into()
            }
        })
        .flex_shrink_0()
        .h(px(if glyph.is_some() {
            ICON_CELL
        } else {
            ROW_HEIGHT
        }))
        .when(glyph.is_some(), |row| {
            row.w(px(ICON_CELL))
                .justify_center()
                .border_1()
                .border_color(rgb(theme::color(if item.checked {
                    theme::PaletteRole::Primary
                } else {
                    theme::PaletteRole::Surface
                })))
        })
        .when(glyph.is_none(), |row| row.w_full().px(px(10.0)))
        .tooltip(move |_, cx| {
            cx.new(|_| MenuTooltip {
                label: tooltip_label.clone(),
            })
            .into()
        })
        .flex()
        .items_center()
        .cursor_pointer()
        .bg(rgb(if menu.selected == index {
            theme::color(theme::PaletteRole::Hover)
        } else {
            theme::color(theme::PaletteRole::Surface)
        }))
        .children(item.swatch.map(|color| {
            div()
                .debug_selector(move || format!("context-menu-swatch-{swatch_label}"))
                .w(px(12.0))
                .h(px(12.0))
                .mr(px(8.0))
                .border_1()
                .border_color(rgb(theme::color(theme::PaletteRole::Muted)))
                .bg(rgb(color))
        }))
        .children(glyph.map(|path| {
            gpui::svg()
                .path(path)
                .size(px(14.0))
                .text_color(rgb(theme::color(theme::PaletteRole::Text)))
        }))
        .children(glyph.is_none().then(|| {
            div()
                .min_w_0()
                .truncate()
                .child(theme::text(label.as_ref()))
        }))
        .children(item.checked.then(|| {
            div()
                .debug_selector(move || format!("context-menu-current-{index}"))
                .when(glyph.is_none(), |marker| {
                    marker.ml_auto().child(theme::text("✓"))
                })
        }))
        .on_click(
            cx.listener(move |owner, _, window, cx| select(owner, action.clone(), window, cx)),
        );
        if glyph.is_some() {
            cells.push(row);
        } else if index > 0 && grid.is_some() {
            trailing.push(row);
        } else {
            panel = panel.child(row);
        }
    }
    if grid.is_some() {
        panel = panel.child(
            div()
                .debug_selector(|| "context-menu-icon-grid".into())
                .w_full()
                .p(px(3.0))
                .border_t_1()
                .border_color(rgb(theme::color(theme::PaletteRole::Muted)))
                .flex()
                .flex_wrap()
                .children(cells),
        );
    }
    panel = panel.children(trailing);
    let glyph_count = grid.map_or(0, Vec::len);
    let is_grid = grid.is_some();
    let selected = menu.selected;
    let count = menu.items.len();
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
                    "left" | "right" if is_grid => {
                        navigate(owner, event.keystroke.key == "right", cx)
                    }
                    "up" | "down" => {
                        let forward = event.keystroke.key == "down";
                        let steps = if !is_grid || selected == 0 || selected > glyph_count {
                            1
                        } else if forward {
                            if selected + ICON_COLUMNS <= glyph_count {
                                ICON_COLUMNS
                            } else {
                                (glyph_count + 1).min(count) - selected
                            }
                        } else {
                            selected.min(ICON_COLUMNS)
                        };
                        for _ in 0..steps {
                            navigate(owner, forward, cx);
                        }
                    }
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

struct MenuTooltip {
    label: gpui::SharedString,
}
impl gpui::Render for MenuTooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .border_1()
            .border_color(rgb(theme::color(theme::PaletteRole::Border)))
            .bg(rgb(theme::color(theme::PaletteRole::Raised)))
            .font_family(theme::FONT_FAMILY)
            .text_xs()
            .text_color(rgb(theme::color(theme::PaletteRole::Text)))
            .child(theme::text(&self.label))
    }
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
                30.0,
                WIDTH
            ),
            point(px(786.0), px(766.0))
        );
        assert_eq!(
            clamped_position(
                point(px(-10.0), px(-20.0)),
                gpui::size(px(1000.0), px(800.0)),
                30.0,
                WIDTH
            ),
            point(px(4.0), px(4.0))
        );
    }
}
