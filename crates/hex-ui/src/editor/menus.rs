//! Native application menus dispatch the editor's existing guarded actions.
use super::{
    AddMidiClip, AddMidiTrack, AddTrack, CancelUpdate, ExportProject, ImportAudio,
    InstallUpdateOnQuit, NewProject, OpenAutomationRecovery, OpenProject, OpenRecentProject,
    OpenRelinkedProject, OpenSettings, PanTimelineLeft, PanTimelineRight, Quit, RecoverKeptSession,
    RecoverPreviousSave, RecoverSavedProject, RecoverUntitledProject, Redo, RemoveTrack,
    SaveAndRestartForUpdate, SaveAsProject, SaveProject, SplitClip, Undo, ZoomIn, ZoomOut,
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
                MenuItem::action(
                    theme::text("open automation recovery file…"),
                    OpenAutomationRecovery,
                ),
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
                MenuItem::separator(),
                MenuItem::action(theme::text("import audio…"), ImportAudio),
                MenuItem::action(theme::text("export wav…"), ExportProject),
            ],
        },
        Menu {
            name: theme::text("edit"),
            items: vec![
                MenuItem::action(theme::text("undo"), Undo),
                MenuItem::action(theme::text("redo"), Redo),
                MenuItem::separator(),
                MenuItem::action(theme::text("split clip"), SplitClip),
            ],
        },
        Menu {
            name: theme::text("track"),
            items: vec![
                MenuItem::action(theme::text("add track"), AddTrack),
                MenuItem::action(theme::text("add midi track"), AddMidiTrack),
                MenuItem::action(theme::text("add midi clip"), AddMidiClip),
                MenuItem::separator(),
                MenuItem::action(theme::text("remove selected track"), RemoveTrack),
            ],
        },
        Menu {
            name: theme::text("view"),
            items: vec![
                MenuItem::action(
                    theme::text("track performance indicators"),
                    super::ToggleTrackPerformance,
                ),
                MenuItem::action(
                    theme::text("track latency indicators"),
                    super::ToggleTrackLatency,
                ),
                MenuItem::separator(),
                MenuItem::action(theme::text("pan left"), PanTimelineLeft),
                MenuItem::action(theme::text("pan right"), PanTimelineRight),
                MenuItem::action(theme::text("zoom in"), ZoomIn),
                MenuItem::action(theme::text("zoom out"), ZoomOut),
            ],
        },
    ]
}

#[cfg(all(test, not(feature = "ui-test-support")))]
mod tests {
    use super::*;

    #[test]
    fn project_menu_keeps_open_actions_and_recent_project_paths() {
        let paths = [
            PathBuf::from("/Work/Live/Session.HEX"),
            PathBuf::from("/Archive/Session.HEX"),
        ];
        let menus = application_menus(&paths);
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
        let recent = file
            .items
            .iter()
            .find_map(|item| match item {
                MenuItem::Submenu(menu) if menu.name == "recent projects" => Some(menu),
                _ => None,
            })
            .expect("recent projects submenu");
        assert_eq!(recent.items.len(), paths.len());
        for ((item, path), label) in recent
            .items
            .iter()
            .zip(&paths)
            .zip(["session.hex · /work/live", "session.hex · /archive"])
        {
            let MenuItem::Action { name, action, .. } = item else {
                panic!("recent project action");
            };
            assert_eq!(name.as_ref(), label);
            let action = action
                .as_any()
                .downcast_ref::<OpenRecentProject>()
                .expect("open recent project");
            assert_eq!(&action.path, path);
        }
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

    #[test]
    fn menus_cover_every_pruned_toolbar_action_with_lowercase_labels() {
        let menus = application_menus(&[]);
        let has_action = |menu_name: &str, check: &dyn Fn(&dyn gpui::Action) -> bool| -> bool {
            let Some(menu) = menus.iter().find(|menu| menu.name == menu_name) else {
                return false;
            };
            menu.items.iter().any(|item| match item {
                MenuItem::Action { name, action, .. } => {
                    assert_eq!(
                        name.as_ref(),
                        theme::text(name.as_ref()).as_ref(),
                        "menu item label must be lowercase: {name}"
                    );
                    check(action.as_ref())
                }
                _ => false,
            })
        };
        assert!(has_action("file", &|a| a.as_any().is::<ImportAudio>()));
        assert!(has_action("file", &|a| a.as_any().is::<ExportProject>()));
        assert!(has_action("edit", &|a| a.as_any().is::<Undo>()));
        assert!(has_action("edit", &|a| a.as_any().is::<Redo>()));
        assert!(has_action("edit", &|a| a.as_any().is::<SplitClip>()));
        assert!(has_action("track", &|a| a.as_any().is::<AddTrack>()));
        assert!(has_action("track", &|a| a.as_any().is::<AddMidiTrack>()));
        assert!(has_action("track", &|a| a.as_any().is::<AddMidiClip>()));
        assert!(has_action("track", &|a| a.as_any().is::<RemoveTrack>()));
        assert!(has_action("view", &|a| a.as_any().is::<PanTimelineLeft>()));
        assert!(has_action("view", &|a| a.as_any().is::<PanTimelineRight>()));
        assert!(has_action("view", &|a| a.as_any().is::<ZoomIn>()));
        assert!(has_action("view", &|a| a.as_any().is::<ZoomOut>()));
    }
}
