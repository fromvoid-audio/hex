//! Native application menus dispatch the editor's existing guarded actions.
use super::{
    NewProject, OpenProject, OpenRecentProject, OpenRelinkedProject, OpenSettings,
    OpenStreamingProject, Quit, RecoverPreviousSave, RecoverSavedProject, RecoverUntitledProject,
    SaveAsProject, SaveProject,
};
use crate::theme;
use gpui::{App, Menu, MenuItem};
use std::path::PathBuf;

pub(super) fn install(cx: &mut App, recent_projects: &[PathBuf]) {
    cx.set_menus(application_menus(recent_projects));
}

fn application_menus(recent_projects: &[PathBuf]) -> Vec<Menu> {
    vec![
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
                MenuItem::action(theme::text("new project"), NewProject),
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
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_project_menu_exposes_one_ordinary_guarded_action() {
        let menus = application_menus(&[]);
        let file = menus.iter().find(|menu| menu.name == "file").unwrap();
        let new_actions: Vec<_> = file
            .items
            .iter()
            .filter_map(|item| match item {
                MenuItem::Action { name, action, .. } if name.starts_with("new ") => {
                    Some((name.as_ref(), action))
                }
                _ => None,
            })
            .collect();
        assert_eq!(new_actions.len(), 1);
        assert_eq!(new_actions[0].0, "new project");
        assert!(new_actions[0].1.as_any().is::<NewProject>());
    }
}
