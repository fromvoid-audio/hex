//! Native application menus dispatch the editor's existing guarded actions.
use super::{
    NewProject, NewStreamingProject, OpenProject, OpenRecentProject, OpenRelinkedProject,
    OpenSettings, OpenStreamingProject, Quit, RecoverPreviousSave, RecoverSavedProject,
    RecoverUntitledProject, SaveAsProject, SaveProject,
};
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
                MenuItem::action(theme::text("new resident project"), NewProject),
                MenuItem::action(
                    theme::text("new verified streaming project"),
                    NewStreamingProject,
                ),
                MenuItem::action(theme::text("open project…"), OpenProject),
                MenuItem::action(
                    theme::text("open as verified streaming…"),
                    OpenStreamingProject,
                ),
                MenuItem::action(theme::text("open and relink media…"), OpenRelinkedProject),
                MenuItem::action(
                    theme::text("recover latest untitled project…"),
                    RecoverUntitledProject,
                ),
                MenuItem::action(
                    theme::text("recover saved project edits…"),
                    RecoverSavedProject,
                ),
                MenuItem::action(theme::text("recover previous save…"), RecoverPreviousSave),
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
