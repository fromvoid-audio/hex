//! Native application menus dispatch the editor's existing guarded actions.
use super::{
    CancelUpdate, InstallUpdateOnQuit, NewProject, OpenProject, OpenRecentProject,
    OpenRelinkedProject, OpenSettings, Quit, RecoverKeptSession, RecoverPreviousSave,
    RecoverSavedProject, RecoverUntitledProject, SaveAndRestartForUpdate, SaveAsProject,
    SaveProject,
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
                MenuItem::action(theme::text("install update on quit"), InstallUpdateOnQuit),
                MenuItem::action(
                    theme::text("save and restart for update"),
                    SaveAndRestartForUpdate,
                ),
                MenuItem::action(theme::text("cancel update"), CancelUpdate),
                MenuItem::separator(),
                MenuItem::action(theme::text("quit hex"), Quit),
            ],
        },
        Menu {
            name: theme::text("file"),
            items: vec![
                MenuItem::action(theme::text("new project"), NewProject),
                MenuItem::action(theme::text("open project…"), OpenProject),
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
                MenuItem::action(theme::text("recover kept session…"), RecoverKeptSession),
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
    fn project_menu_exposes_one_ordinary_new_and_open_action() {
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
        let open_actions: Vec<_> = file
            .items
            .iter()
            .filter_map(|item| match item {
                MenuItem::Action { name, action, .. } if action.as_any().is::<OpenProject>() => {
                    Some(name.as_ref())
                }
                _ => None,
            })
            .collect();
        assert_eq!(open_actions, ["open project…"]);
        for item in &file.items {
            if let MenuItem::Action { name, action, .. } = item {
                assert!(
                    !name.contains("resident") && !name.contains("streaming"),
                    "storage mechanisms must stay internal: {name}"
                );
                assert!(!action.as_any().is::<super::super::NewStreamingProject>());
                assert!(!action.as_any().is::<super::super::OpenStreamingProject>());
            }
        }
    }
}
