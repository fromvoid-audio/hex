//! Native application menus dispatch the editor's existing guarded actions.
use super::{OpenProject, OpenRecentProject, OpenSettings, Quit, SaveAsProject, SaveProject};
use crate::theme;
use gpui::{App, Menu, MenuItem};
use std::path::PathBuf;

pub(super) fn install(cx: &mut App, recent_projects: &[PathBuf]) {
    cx.set_menus(vec![
        Menu {
            name: theme::text("hex"),
            items: vec![
                MenuItem::action(theme::text("settings…"), OpenSettings),
                MenuItem::separator(),
                MenuItem::action(theme::text("quit hex"), Quit),
            ],
        },
        Menu {
            name: theme::text("file"),
            items: vec![
                MenuItem::action(theme::text("open project…"), OpenProject),
                MenuItem::submenu(Menu {
                    name: theme::text("recent projects"),
                    items: recent_projects
                        .iter()
                        .map(|path| {
                            MenuItem::action(
                                theme::text(super::recent_projects::recent_project_label(
                                    path,
                                    recent_projects,
                                )),
                                OpenRecentProject { path: path.clone() },
                            )
                        })
                        .collect(),
                }),
                MenuItem::separator(),
                MenuItem::action(theme::text("save"), SaveProject),
                MenuItem::action(theme::text("save as…"), SaveAsProject),
            ],
        },
    ]);
}
