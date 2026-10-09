//! Native application menus dispatch the editor's existing guarded actions.
use super::{OpenProject, Quit, SaveAsProject, SaveProject};
use crate::theme;
use gpui::{App, Menu, MenuItem};

pub(super) fn install(cx: &mut App) {
    cx.set_menus(vec![
        Menu {
            name: theme::text("hex"),
            items: vec![MenuItem::action(theme::text("quit hex"), Quit)],
        },
        Menu {
            name: theme::text("file"),
            items: vec![
                MenuItem::action(theme::text("open project…"), OpenProject),
                MenuItem::separator(),
                MenuItem::action(theme::text("save"), SaveProject),
                MenuItem::action(theme::text("save as…"), SaveAsProject),
            ],
        },
    ]);
}
