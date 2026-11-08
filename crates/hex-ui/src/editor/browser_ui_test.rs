use super::{
    BrowserSection, EditorView,
    drag::{BrowserAudioDrag, BrowserDropDestination, ClipDrag, ClipDropDestination, ClipPosition},
};
use gpui::{
    AppContext, Empty, Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px,
};
use hex_library::{DirectoryListing, EntryKind, LibraryEntry};
use hex_project::{Command, Created, Editor, Project, TrackId};
use hex_session::service::{Service, ViewState};
use std::{cell::RefCell, rc::Rc, sync::Arc};

struct PreferencesTestDirectory(std::path::PathBuf);

impl PreferencesTestDirectory {
    fn new() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "hex-ui-browser-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).expect("preferences test directory");
        Self(path)
    }
}

impl Drop for PreferencesTestDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn mono_pcm_wav(frame_count: usize) -> Vec<u8> {
    let samples = vec![4_000_i16; frame_count];
    let data = samples
        .iter()
        .flat_map(|sample| sample.to_le_bytes())
        .collect::<Vec<_>>();
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36_u32 + data.len() as u32).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&48_000_u32.to_le_bytes());
    bytes.extend_from_slice(&96_000_u32.to_le_bytes());
    bytes.extend_from_slice(&2_u16.to_le_bytes());
    bytes.extend_from_slice(&16_u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&(data.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&data);
    bytes
}

fn browser_drag_fixture(
    cx: &mut TestAppContext,
) -> (
    gpui::AnyWindowHandle,
    gpui::Entity<EditorView>,
    TrackId,
    PreferencesTestDirectory,
    std::path::PathBuf,
) {
    let (handle, editor, tracks, directory, path) =
        browser_drag_fixture_with_tracks(cx, &["target"]);
    (handle, editor, tracks[0], directory, path)
}

#[gpui::test]
fn consumed_browser_search_key_is_redacted_from_action_diagnostics(cx: &mut TestAppContext) {
    let (handle, editor, _, directory, _) = browser_drag_fixture(cx);
    let mut log = hex_diagnostics::SessionLog::start(directory.0.join("diagnostics")).unwrap();
    cx.update(|cx| {
        crate::diagnostics::install_session_log(cx, Ok(log.handle()));
        crate::diagnostics::install_action_diagnostics(cx, true);
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, cx| editor.read(cx).browser.search.read(cx).focus(window));
    visual.simulate_keystrokes("q->密");
    editor.read_with(&visual, |view, cx| {
        assert_eq!(view.browser.search.read(cx).text(), "密");
    });
    log.shutdown().unwrap();
    let contents = std::fs::read_to_string(log.path()).unwrap();
    assert!(contents.contains("stage=key key=\\\"redacted\\\""));
    assert!(!contents.contains("密"));
}

fn browser_drag_fixture_with_tracks(
    cx: &mut TestAppContext,
    names: &[&str],
) -> (
    gpui::AnyWindowHandle,
    gpui::Entity<EditorView>,
    Vec<TrackId>,
    PreferencesTestDirectory,
    std::path::PathBuf,
) {
    let directory = PreferencesTestDirectory::new();
    let path = directory.0.join("loop.WAV");
    std::fs::write(&path, mono_pcm_wav(4_800)).expect("wav fixture");
    let mut model = Editor::new(Project::new(48_000).expect("project"), 4).expect("editor");
    let mut tracks = Vec::with_capacity(names.len());
    for name in names {
        let Created::Track(track) = model
            .execute(Command::AddTrack {
                name: (*name).into(),
            })
            .expect("add track")
        else {
            panic!("track result")
        };
        tracks.push(track);
    }
    let bootstrap = cx.add_window(|_, _| Empty);
    let handle = *bootstrap;
    let editor = cx
        .update_window(handle, |_, window, cx| {
            let service = Service::new(48_000, 32, 8_000_000).expect("test service");
            let editor = window.replace_root(cx, |window, cx| {
                EditorView::new(window, cx, service, Err("offline ui test".into()))
            });
            editor.update(cx, |view, cx| {
                // Fixed geometry for pointer/drop tests; the normal navigation
                // allowance is tested by the arranger navigation regressions.
                view.viewport = Some(super::super::viewport::Viewport::fit(384_000));
                view.state = Some(Arc::new(ViewState {
                    canonical_audio_source: None,
                    comparison_audio: None,
                    project: Arc::new(model.project().clone()),
                    waveforms: Default::default(),
                    source_overviews: Default::default(),
                    audio_clip_waveforms: std::collections::BTreeMap::new(),
                    midi_preview: Arc::new(Default::default()),
                    gain_preview: Ok(Arc::new(hex_engine::GainAutomationPreview::default())),
                    timeline_frames: 384_000,
                    can_undo: false,
                    can_redo: false,
                    revision: 1,
                    unsaved: false,
                    document_path: None,
                }));
                view.browser.scans.generation = view.browser.scans.generation.saturating_add(1);
                view.browser.places = vec![directory.0.clone()];
                view.browser.directory = directory.0.clone();
                // This fixture installs a validated snapshot, replacing startup work.
                view.browser.loading = false;
                view.browser.error = None;
                view.browser.listing = Some(DirectoryListing {
                    directory: directory.0.clone(),
                    entries: vec![LibraryEntry {
                        name: "loop.WAV".into(),
                        path: path.clone(),
                        kind: EntryKind::WaveAudio,
                    }],
                    enumerated_entries: 1,
                    truncated: false,
                });
                view.browser.samples.replace(
                    hex_library::catalog::scan_samples(std::slice::from_ref(&directory.0)).catalog,
                );
                view.browser.section = BrowserSection::Place(directory.0.clone());
                cx.notify();
            });
            editor
        })
        .expect("editor window");
    (handle, editor, tracks, directory, path)
}

#[gpui::test]
fn command_f_opens_all_search_and_replaces_query_from_editor_and_results(cx: &mut TestAppContext) {
    cx.update(super::super::bind_editor_keys);
    let (handle, editor, track, _directory, _) = browser_drag_fixture(cx);
    cx.update_window(handle, |_, window, cx| {
        editor.update(cx, |view, cx| {
            view.browser.open = false;
            view.browser.section = BrowserSection::Projects;
            view.browser
                .listing
                .as_mut()
                .expect("listing")
                .entries
                .push(LibraryEntry {
                    name: "Loop.hex".into(),
                    path: view.browser.directory.join("Loop.hex"),
                    kind: EntryKind::Project,
                });
            view.focus.focus(window);
            cx.notify();
        });
    })
    .expect("focus editor");

    cx.simulate_keystrokes(handle, "cmd-f");
    cx.update_window(handle, |_, window, cx| {
        editor.read_with(cx, |view, cx| {
            assert!(view.browser.open);
            assert_eq!(view.browser.section, BrowserSection::All);
            assert!(
                view.browser
                    .search
                    .read(cx)
                    .focus_handle()
                    .is_focused(window)
            );
            assert!(!view.browser.results_focused);
        });
    })
    .expect("search focus");
    cx.simulate_input(handle, "LOOP");
    editor.read_with(cx, |view, cx| {
        assert_eq!(
            view.browser_visible_entries(cx).len(),
            2,
            "all includes samples and projects"
        );
        assert!(view.browser_visible_devices(cx).is_empty());
    });

    cx.simulate_keystrokes(handle, "down");
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.browser.selected_index, 0,
            "first down selects first match"
        )
    });
    cx.simulate_keystrokes(handle, "down");
    editor.read_with(cx, |view, _| assert_eq!(view.browser.selected_index, 1));
    cx.simulate_keystrokes(handle, "cmd-f");
    cx.simulate_input(handle, "utility");
    editor.read_with(cx, |view, cx| {
        assert_eq!(view.browser.search.read(cx).text(), "utility");
        assert!(view.browser_visible_entries(cx).is_empty());
        let devices = view.browser_visible_devices(cx);
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].1.name.to_lowercase(), "utility");
        assert_eq!(view.browser_selected_label(cx).to_lowercase(), "utility");
        assert!(!view.browser.results_focused);
    });
    cx.simulate_keystrokes(handle, "down");
    editor.read_with(cx, |view, _| assert_eq!(view.browser.selected_index, 0));

    cx.update_window(handle, |_, window, cx| {
        editor.update(cx, |view, cx| {
            view.mode = super::super::EditorMode::Session;
            view.open_track_context_menu(point(px(100.0), px(100.0)), Some(track), window, cx);
        });
    })
    .expect("open context menu");
    cx.simulate_keystrokes(handle, "cmd-f");
    editor.read_with(cx, |view, _| assert!(view.context_menu.is_none()));
    cx.simulate_input(handle, "écho");
    cx.simulate_keystrokes(handle, "cmd-f");
    cx.simulate_input(handle, "loop");
    editor.read_with(cx, |view, cx| {
        assert_eq!(view.browser.search.read(cx).text(), "loop")
    });
    cx.simulate_keystrokes(handle, "escape");
    editor.read_with(cx, |view, cx| {
        assert!(view.browser.search.read(cx).text().is_empty())
    });
}

#[gpui::test]
fn command_f_commits_clip_name_draft_then_routes_typing_to_browser(cx: &mut TestAppContext) {
    cx.update(super::super::bind_editor_keys);
    let (project, _, _, clip) = crate::editor::clip_drag_ui_test::audio_project();
    let (handle, editor) =
        crate::editor::clip_drag_ui_test::editor_window(cx, project, Arc::default());
    cx.update_window(handle, |_, window, cx| {
        editor.update(cx, |view, cx| {
            view.selected_clip = Some(clip);
            view.begin_clip_name(window, cx);
        });
    })
    .expect("rename focus");
    cx.simulate_input(handle, "verse");
    cx.simulate_keystrokes(handle, "cmd-f");
    cx.simulate_input(handle, "utility");
    editor.read_with(cx, |view, cx| {
        assert!(view.clip_name_edit.is_none());
        assert!(matches!(&view.last_enqueued_edit,
            Some(Command::RenameClip { clip: target, name }) if *target == clip && name == "verse"));
        assert_eq!(view.browser.search.read(cx).text(), "utility");
        assert_eq!(view.browser.section, BrowserSection::All);
    });
}

#[gpui::test]
fn command_f_replaces_entire_query_during_ime_composition(cx: &mut TestAppContext) {
    use gpui::EntityInputHandler;
    cx.update(super::super::bind_editor_keys);
    let (handle, editor, _, _directory, _) = browser_drag_fixture(cx);
    cx.simulate_keystrokes(handle, "cmd-f");
    cx.simulate_input(handle, "loop");
    cx.update_window(handle, |_, window, cx| {
        let search = editor.read(cx).browser.search.clone();
        search.update(cx, |input, cx| {
            input.replace_and_mark_text_in_range(None, "音", Some(1..1), window, cx);
            assert!(input.marked_text_range(window, cx).is_some());
        });
    })
    .unwrap();
    cx.simulate_keystrokes(handle, "cmd-f");
    cx.simulate_input(handle, "utility");
    editor.read_with(cx, |view, cx| {
        assert_eq!(view.browser.search.read(cx).text(), "utility");
    });
}

#[gpui::test]
fn command_f_caret_movement_preserves_result_scroll(cx: &mut TestAppContext) {
    cx.update(super::super::bind_editor_keys);
    let (handle, editor, _, directory, _) = browser_drag_fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.simulate_keystrokes("cmd-f");
    visual.simulate_input("sample");
    for index in 0..50 {
        std::fs::write(
            directory.0.join(format!("sample-{index}.wav")),
            mono_pcm_wav(16),
        )
        .unwrap();
    }
    editor.update(visual, |view, cx| view.refresh_sample_catalog(cx));
    visual.run_until_parked();
    editor.update(visual, |view, cx| {
        view.browser
            .scroll
            .scroll_to_item_strict(20, gpui::ScrollStrategy::Top);
        cx.notify();
    });
    visual.run_until_parked();
    let first_device = "browser-device-instrument-hex.instrument.sampler";
    assert!(visual.debug_bounds(first_device).is_none());
    assert!(visual.debug_bounds("browser-entry-20").is_some());
    assert!(visual.debug_bounds("browser-entry-0").is_none());
    visual.simulate_keystrokes("left shift-left");
    visual.run_until_parked();
    assert!(
        visual.debug_bounds("browser-entry-20").is_some(),
        "caret movement must preserve viewport"
    );
    assert!(visual.debug_bounds("browser-entry-0").is_none());
    visual.simulate_keystrokes("end backspace");
    visual.run_until_parked();
    assert!(
        visual.debug_bounds(first_device).is_some(),
        "changed query reveals its first device result"
    );
    editor.read_with(visual, |view, cx| {
        assert_eq!(view.browser.search.read(cx).text(), "sampl");
        assert!(!view.browser.results_focused);
    });
}

#[gpui::test]
fn command_f_device_highlight_waits_for_result_focus(cx: &mut TestAppContext) {
    use crate::theme;
    cx.update(super::super::bind_editor_keys);
    let (handle, _, _, _directory, _) = browser_drag_fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.simulate_keystrokes("cmd-f");
    visual.simulate_input("utility");
    visual.run_until_parked();
    let selector = "browser-device-effect-hex.utility";
    assert_eq!(
        visual.rendered_background(selector),
        Some(gpui::rgb(theme::color(theme::PaletteRole::Surface)).into())
    );
    visual.simulate_keystrokes("down");
    visual.run_until_parked();
    assert_eq!(
        visual.rendered_background(selector),
        Some(gpui::rgb(theme::color(theme::PaletteRole::Secondary)).into())
    );
}

#[gpui::test]
fn command_f_category_click_keeps_query_but_moves_real_focus_in_session(cx: &mut TestAppContext) {
    cx.update(super::super::bind_editor_keys);
    let (handle, editor, _, _directory, _) = browser_drag_fixture(cx);
    editor.update(cx, |view, cx| {
        view.mode = crate::editor::EditorMode::Session;
        cx.notify();
    });
    let visual = &mut VisualTestContext::from_window(handle, cx);
    for category in ["samples", "devices", "projects", "all"] {
        visual.simulate_keystrokes("cmd-f");
        visual.simulate_input("utilit");
        visual.run_until_parked();
        let selector = Box::leak(format!("browser-section-{category}").into_boxed_str());
        let bounds = visual.debug_bounds(selector).expect("category");
        visual.simulate_mouse_down(bounds.center(), MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(bounds.center(), MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        visual.update(|window, cx| {
            editor.read_with(cx, |view, cx| {
                assert_eq!(
                    view.browser_key_context(window, cx),
                    "hex_browser",
                    "category {category}"
                )
            });
        });
        visual.simulate_input("y");
        editor.read_with(visual, |view, cx| {
            assert_eq!(view.browser.search.read(cx).text(), "utilit")
        });
        let field = visual.debug_bounds("browser-search").unwrap();
        visual.simulate_click(field.center(), Modifiers::default());
        visual.simulate_keystrokes("right");
        visual.simulate_input("y");
        editor.read_with(visual, |view, cx| {
            assert_eq!(view.browser.search.read(cx).text(), "utility")
        });
    }
    visual.simulate_keystrokes("down enter");
    editor.read_with(visual, |view, _| {
        assert!(view.status.contains("drag a browser device"));
        assert!(view.last_transport_command.is_none());
    });
}

#[gpui::test]
fn command_f_escape_keeps_navigation_until_explicit_text_entry(cx: &mut TestAppContext) {
    cx.update(super::super::bind_editor_keys);
    let (handle, editor, _, _directory, _) = browser_drag_fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.simulate_keystrokes("cmd-f down right");
    visual.run_until_parked();
    visual.simulate_keystrokes("down");
    let generation = editor.read_with(visual, |view, _| {
        assert!(view.browser.results_focused);
        assert_eq!(
            view.browser.playback_generation, 1,
            "Down auto-auditions the sample"
        );
        view.browser.playback_generation
    });
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    visual.update(|window, cx| {
        editor.read_with(cx, |view, cx| {
            assert!(!view.browser.results_focused);
            assert_eq!(view.browser_key_context(window, cx), "hex_browser");
            assert!(
                !view
                    .browser
                    .search
                    .read(cx)
                    .focus_handle()
                    .is_focused(window)
            );
        });
    });
    // Unbound printable keys leave result navigation untouched. `r` now has
    // its explicit random-sample action in this context.
    visual.simulate_input("text");
    editor.read_with(visual, |view, cx| {
        assert!(view.browser.search.read(cx).text().is_empty());
        assert_eq!(view.browser.playback_generation, generation);
    });
    let field = visual.debug_bounds("browser-search").unwrap();
    visual.simulate_click(field.center(), Modifiers::default());
    visual.simulate_keystrokes("right");
    visual.simulate_input("loop");
    editor.read_with(visual, |view, cx| {
        assert_eq!(
            view.browser.playback_generation, generation,
            "Right in the search field must only move the caret"
        );
        assert_eq!(view.browser.search.read(cx).text(), "loop");
    });
}

#[gpui::test]
fn command_f_scan_completion_preserves_selected_device(cx: &mut TestAppContext) {
    cx.update(super::super::bind_editor_keys);
    let (handle, editor, _, _directory, _) = browser_drag_fixture(cx);
    cx.simulate_keystrokes(handle, "cmd-f");
    cx.simulate_input(handle, "utility");
    let request = editor.update(cx, |view, cx| {
        view.browser.listing = None;
        view.browser.loading = true;
        view.browser.scans = Default::default();
        let request = view
            .browser
            .scans
            .request(view.browser.directory.clone())
            .unwrap();
        cx.notify();
        request
    });
    cx.simulate_keystrokes(handle, "down");
    editor.read_with(cx, |view, cx| {
        assert_eq!(view.browser_selected_label(cx).to_lowercase(), "utility")
    });
    editor.update(cx, |view, cx| {
        let directory = view.browser.directory.clone();
        view.browser_scan_finished(
            request,
            Ok(DirectoryListing {
                directory: directory.clone(),
                entries: vec![LibraryEntry {
                    name: "utility.hex".into(),
                    path: directory.join("utility.hex"),
                    kind: EntryKind::Project,
                }],
                enumerated_entries: 1,
                truncated: false,
            }),
            cx,
        );
        cx.notify();
    });
    editor.read_with(cx, |view, cx| {
        assert_eq!(
            view.browser_selected_label(cx).to_lowercase(),
            "utility",
            "late files must not retarget device selection"
        );
        assert_eq!(view.browser.selected_index, 0);
    });
    cx.simulate_keystrokes(handle, "enter");
    editor.read_with(cx, |view, _| {
        assert!(view.status.contains("drag a browser device"))
    });
}

#[gpui::test]
fn command_f_device_click_retains_browser_keys_in_session(cx: &mut TestAppContext) {
    cx.update(super::super::bind_editor_keys);
    let (handle, editor, _, _directory, _) = browser_drag_fixture(cx);
    editor.update(cx, |view, cx| {
        view.mode = crate::editor::EditorMode::Session;
        cx.notify();
    });
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.simulate_keystrokes("cmd-f");
    visual.simulate_input("utility");
    visual.run_until_parked();
    let row = visual
        .debug_bounds("browser-device-effect-hex.utility")
        .expect("utility result");
    visual.simulate_mouse_down(row.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(row.center(), MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    visual.update(|window, cx| {
        editor.read_with(cx, |view, cx| {
            assert_eq!(view.browser_key_context(window, cx), "hex_browser")
        });
    });
    visual.simulate_keystrokes("enter escape");
    editor.read_with(visual, |view, cx| {
        assert!(
            view.last_transport_command.is_none(),
            "browser enter must not launch session playback"
        );
        assert!(view.status.contains("drag a browser device"));
        assert!(view.browser.search.read(cx).text().is_empty());
    });
}

#[gpui::test]
fn command_f_category_switch_scrolls_first_result_into_view(cx: &mut TestAppContext) {
    cx.update(super::super::bind_editor_keys);
    let (handle, editor, _, directory, _) = browser_drag_fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.simulate_keystrokes("cmd-f");
    for index in 0..50 {
        std::fs::write(
            directory.0.join(format!("sample-{index}.wav")),
            mono_pcm_wav(16),
        )
        .unwrap();
        std::fs::write(directory.0.join(format!("project-{index}.hex")), []).unwrap();
    }
    editor.update(visual, |view, cx| {
        view.refresh_sample_catalog(cx);
        view.request_browser_scan(directory.0.clone(), cx);
    });
    visual.run_until_parked();
    let samples = visual.debug_bounds("browser-section-samples").unwrap();
    visual.simulate_mouse_down(samples.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(samples.center(), MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let root = visual.debug_bounds("browser-disclosure-0").unwrap();
    visual.simulate_mouse_down(root.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(root.center(), MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.update(visual, |view, cx| {
        view.browser
            .scroll
            .scroll_to_item_strict(20, gpui::ScrollStrategy::Top);
        cx.notify();
    });
    visual.run_until_parked();
    assert!(
        visual.debug_bounds("browser-entry-20").is_some(),
        "scroll target is painted"
    );
    assert!(
        visual.debug_bounds("browser-entry-0").is_none(),
        "first row is outside the painted viewport"
    );
    let projects = visual.debug_bounds("browser-section-projects").unwrap();
    visual.simulate_mouse_down(projects.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(projects.center(), MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!(
        visual.debug_bounds("browser-entry-0").is_some(),
        "new category's first result must be visible"
    );
}

#[gpui::test]
fn browser_search_owns_typing_and_blocks_editor_shortcuts(cx: &mut TestAppContext) {
    cx.update(super::super::bind_editor_keys);
    let bootstrap = cx.add_window(|_, _| Empty);
    let handle = *bootstrap;
    let editor = cx
        .update_window(handle, |_, window, cx| {
            let service = Service::new(48_000, 32, 8_000_000).expect("disposable test service");
            let editor = window.replace_root(cx, |window, cx| {
                EditorView::new(window, cx, service, Err("offline test".into()))
            });
            editor.update(cx, |view, cx| {
                let project = Editor::new(Project::new(48_000).expect("test project"), 4)
                    .expect("test editor")
                    .project()
                    .clone();
                view.state = Some(Arc::new(ViewState {
                    canonical_audio_source: None,
                    comparison_audio: None,
                    project: Arc::new(project),
                    waveforms: Default::default(),
                    source_overviews: Default::default(),
                    audio_clip_waveforms: std::collections::BTreeMap::new(),
                    midi_preview: Arc::new(Default::default()),
                    gain_preview: Ok(Arc::new(hex_engine::GainAutomationPreview::default())),
                    timeline_frames: 0,
                    can_undo: false,
                    can_redo: false,
                    revision: 0,
                    unsaved: false,
                    document_path: None,
                }));
                view.browser
                    .search
                    .update(cx, |input, _| input.focus(window));
                cx.notify();
            });
            editor
        })
        .expect("editor window");

    cx.simulate_keystrokes(handle, "shift-a space cmd-shift-t");

    editor.read_with(cx, |view, cx| {
        assert_eq!(view.browser.search.read(cx).text(), "a ");
        assert!(
            view.pending_requests.is_empty(),
            "search focus must suppress add-track"
        );
        assert!(
            view.pending_audio_commands.is_empty(),
            "search focus must suppress transport actions"
        );
        assert_eq!(
            view.state
                .as_ref()
                .map(|state| state.project.tracks().len()),
            Some(0)
        );
    });
}

#[gpui::test]
fn device_browser_lists_registered_devices_and_drag_starts_without_track_focus(
    cx: &mut TestAppContext,
) {
    let bootstrap = cx.add_window(|_, _| Empty);
    let handle = *bootstrap;
    let editor = cx
        .update_window(handle, |_, window, cx| {
            let service = Service::new(48_000, 32, 8_000_000).expect("service");
            let editor = window.replace_root(cx, |window, cx| {
                EditorView::new(window, cx, service, Err("offline test".into()))
            });
            editor.update(cx, |view, cx| {
                let project = Project::new(48_000).expect("project");
                view.state = Some(Arc::new(ViewState {
                    canonical_audio_source: None,
                    comparison_audio: None,
                    project: Arc::new(project),
                    waveforms: Default::default(),
                    source_overviews: Default::default(),
                    audio_clip_waveforms: std::collections::BTreeMap::new(),
                    midi_preview: Arc::new(Default::default()),
                    gain_preview: Ok(Arc::new(hex_engine::GainAutomationPreview::default())),
                    timeline_frames: 0,
                    can_undo: false,
                    can_redo: false,
                    revision: 0,
                    unsaved: false,
                    document_path: None,
                }));
                view.browser.section = BrowserSection::Instruments;
                view.clip_view_open = false;
                view.device_panel_active = false;
                cx.notify();
            });
            editor
        })
        .expect("editor window");
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let instrument = visual
        .debug_bounds("browser-device-instrument-hex.instrument.sine")
        .expect("registered sine instrument row");
    assert!(
        visual
            .debug_bounds("browser-device-effect-hex.utility")
            .is_some()
    );
    assert!(
        visual
            .debug_bounds("browser-device-instrument-hex.instrument.operator")
            .is_some()
    );

    let down = point(
        instrument.origin.x + instrument.size.width / 2.0,
        instrument.origin.y + instrument.size.height / 2.0,
    );
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(down.x + px(12.0), down.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    assert!(cx.update(|app| app.has_active_drag()));
    assert!(visual.debug_bounds("device-panel-empty").is_some());
    editor.read_with(cx, |view, _| {
        assert!(view.selected_track.is_none());
        assert!(view.clip_view_open, "browser drag reveals the closed pane");
        assert!(
            view.device_panel_active,
            "browser drag selects device details"
        );
        let drag = view
            .active_native_device_drag
            .as_ref()
            .expect("browser device drag begins without a focused track");
        assert!(matches!(
            drag.source,
            super::drag::NativeDeviceDragSource::Browser {
                class: hex_engine::NativeDeviceClass::Instrument,
                ..
            }
        ));
        assert!(drag.destination.get().is_none());
    });
    visual.simulate_mouse_up(down, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(view.active_native_device_drag.is_none());
        assert!(view.clip_view_open, "a no-target drop leaves the pane open");
        assert!(view.device_panel_active);
        assert!(
            view.selected_track.is_none(),
            "drag does not invent a track"
        );
    });
}

#[gpui::test]
fn browser_device_drag_reveals_devices_and_preserves_clip_selection_on_cancel(
    cx: &mut TestAppContext,
) {
    let (project, track, _, clip) = super::super::clip_drag_ui_test::audio_project();
    let (handle, editor) =
        super::super::clip_drag_ui_test::editor_window(cx, project, Arc::default());
    editor.update(cx, |view, cx| {
        view.selected_track = Some(track);
        view.selected_clip = Some(clip);
        view.device_panel_active = false;
        view.clip_view_open = true;
        view.browser.section = BrowserSection::Instruments;
        cx.notify();
    });

    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    assert!(visual.debug_bounds("arrangement-audio-clip-view").is_some());
    let instrument = visual
        .debug_bounds("browser-device-instrument-hex.instrument.sine")
        .expect("registered sine instrument row");
    let down = point(instrument.center().x, instrument.center().y);
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(down.x + px(12.0), down.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();

    assert!(visual.debug_bounds("device-panel").is_some());
    editor.read_with(cx, |view, _| {
        assert!(view.clip_view_open);
        assert!(view.device_panel_active);
        assert_eq!(view.selected_track, Some(track));
        assert_eq!(view.selected_clip, Some(clip));
        assert!(view.active_native_device_drag.is_some());
    });

    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(view.active_native_device_drag.is_none());
        assert!(view.clip_view_open, "cancel keeps the revealed pane open");
        assert!(view.device_panel_active);
        assert_eq!(view.selected_track, Some(track));
        assert_eq!(view.selected_clip, Some(clip));
    });
}

#[gpui::test]
fn browser_project_open_queues_selected_path_without_second_click_confirmation(
    cx: &mut TestAppContext,
) {
    let bootstrap = cx.add_window(|_, _| Empty);
    let handle = *bootstrap;
    let editor = cx
        .update_window(handle, |_, window, cx| {
            let service = Service::new(48_000, 32, 8_000_000).expect("disposable test service");
            let editor = window.replace_root(cx, |window, cx| {
                EditorView::new(window, cx, service, Err("offline test".into()))
            });
            editor.update(cx, |view, cx| {
                let project = Editor::new(Project::new(48_000).expect("test project"), 4)
                    .expect("test editor")
                    .project()
                    .clone();
                view.state = Some(Arc::new(ViewState {
                    canonical_audio_source: None,
                    comparison_audio: None,
                    project: Arc::new(project),
                    waveforms: Default::default(),
                    source_overviews: Default::default(),
                    audio_clip_waveforms: std::collections::BTreeMap::new(),
                    midi_preview: Arc::new(Default::default()),
                    gain_preview: Ok(Arc::new(hex_engine::GainAutomationPreview::default())),
                    timeline_frames: 0,
                    can_undo: true,
                    can_redo: false,
                    revision: 1,
                    unsaved: true,
                    document_path: None,
                }));
                cx.notify();
            });
            editor
        })
        .expect("editor window");
    let first = std::path::PathBuf::from("/tmp/first.hex");
    let second = std::path::PathBuf::from("/tmp/second.hex");

    cx.update_window(handle, |_, _, cx| {
        editor.update(cx, |view, cx| {
            view.open_browser_project(first.clone(), cx);
            assert_eq!(view.pending_open_path.as_ref(), Some(&first));
            assert!(view.pending_requests.is_empty());
        });
    })
    .expect("editor window");
    assert!(
        cx.has_pending_prompt(),
        "the first selection reaches confirmation"
    );
    cx.update_window(handle, |_, _, cx| {
        editor.update(cx, |view, cx| {
            view.open_browser_project(second.clone(), cx);
            assert!(!view.opening_project);
            assert!(view.pending_requests.is_empty());
            assert!(
                view.error.is_some(),
                "another path cannot replace an active intent"
            );
        });
    })
    .expect("editor window");
    cx.simulate_prompt_answer("cancel");
    cx.run_until_parked();
    cx.update_window(handle, |_, _, cx| {
        editor.update(cx, |view, cx| {
            assert!(!view.project_open.busy());
            assert!(view.pending_open_path.is_none());
            view.open_browser_project(second.clone(), cx);
            assert_eq!(view.pending_open_path.as_ref(), Some(&second));
            assert!(view.pending_requests.is_empty());
        });
    })
    .expect("editor window");
    assert!(cx.has_pending_prompt());
    cx.simulate_prompt_answer("cancel");
    cx.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(!view.opening_project);
        assert!(!view.project_open.busy());
        assert!(view.pending_open_path.is_none());
        assert!(view.state.as_ref().unwrap().unsaved);
        assert!(view.pending_requests.is_empty());
    });
}

#[gpui::test]
fn browser_right_edge_drag_resizes_and_release_stops_resizing(cx: &mut TestAppContext) {
    use gpui::{Modifiers, MouseButton, VisualTestContext, point, px};
    let bootstrap = cx.add_window(|_, _| Empty);
    let handle = *bootstrap;
    let editor = cx
        .update_window(handle, |_, window, cx| {
            let service = Service::new(48_000, 32, 8_000_000).expect("test service");
            window.replace_root(cx, |window, cx| {
                EditorView::new(window, cx, service, Err("offline test".into()))
            })
        })
        .expect("editor window");
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    // Grab the actual separator; the browser may use a compact or expanded width.
    let separator = visual
        .debug_bounds("browser-resize-handle")
        .expect("browser separator");
    visual.simulate_mouse_down(separator.center(), MouseButton::Left, Modifiers::default());
    editor.read_with(cx, |view, _| {
        assert!(view.browser.resizing, "separator must receive mouse down")
    });
    visual.simulate_mouse_move(
        point(px(310.0), px(200.0)),
        MouseButton::Left,
        Modifiers::default(),
    );
    editor.read_with(cx, |view, _| assert_eq!(view.browser.width, 310.0));
    visual.simulate_mouse_move(
        point(px(240.0), px(200.0)),
        MouseButton::Left,
        Modifiers::default(),
    );
    editor.read_with(cx, |view, _| assert_eq!(view.browser.width, 240.0));
    visual.simulate_mouse_up(
        point(px(240.0), px(200.0)),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.simulate_mouse_move(point(px(300.0), px(200.0)), None, Modifiers::default());
    editor.read_with(cx, |view, _| {
        assert!(!view.browser.resizing);
        assert_eq!(view.browser.width, 240.0);
    });
}

#[gpui::test]
fn browser_audio_drag_floats_outside_lanes_locks_preview_and_imports_exact_candidate(
    cx: &mut TestAppContext,
) {
    let (handle, editor, track, _directory, path) = browser_drag_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let source = visual
        .debug_bounds("browser-entry-0")
        .expect("rendered wav browser row");
    let lane_selector = Box::leak(format!("arrangement-lane-{}", track.value()).into_boxed_str());
    let lane = visual
        .debug_bounds(lane_selector)
        .expect("rendered arrangement target lane");
    let down = point(
        source.origin.x + source.size.width / 2.0,
        source.origin.y + source.size.height / 2.0,
    );
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(down.x + px(12.0), down.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    assert!(cx.update(|app| app.has_active_drag()));
    let floating = visual
        .debug_bounds("browser-audio-floating-preview")
        .expect("outside-lane drag has cursor ghost");
    assert!(f32::from(floating.size.width) > 100.0);
    crate::editor::clip_boundary::tests::assert_open_upper_right(
        &mut visual,
        "browser-audio-floating-preview",
        "browser-audio-floating-preview-trailing",
        Some("browser-drag-name"),
    );
    let title = visual.debug_bounds("browser-drag-name").unwrap();
    assert_eq!(
        title.size.height,
        px(crate::editor::clip_name::CLIP_NAME_HEIGHT)
    );

    editor.read_with(cx, |view, _| {
        let drag = view
            .active_browser_audio_drag
            .as_ref()
            .expect("browser drag active");
        assert!(!drag.lane_hovered.get());
        assert!(drag.preview.get().is_none());
    });

    let outside = point(
        source.origin.x + source.size.width / 2.0,
        source.origin.y + source.size.height / 2.0,
    );
    visual.simulate_mouse_move(outside, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let drag = view
            .active_browser_audio_drag
            .as_ref()
            .expect("drag remains active");
        assert!(drag.preview.get().is_none());
    });

    let target = point(
        lane.origin.x + lane.size.width * 0.58,
        lane.origin.y + lane.size.height / 2.0,
    );
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    for _ in 0..8 {
        if editor.read_with(cx, |view, _| {
            view.active_browser_audio_drag
                .as_ref()
                .is_some_and(|drag| drag.preview.get().is_some())
        }) {
            break;
        }
        visual.run_until_parked();
    }
    let expected = editor.read_with(cx, |view, _| {
        let drag = view
            .active_browser_audio_drag
            .as_ref()
            .expect("drag remains active");
        assert!(drag.lane_hovered.get());
        let preview = drag
            .preview
            .get()
            .expect("prepared preview is exact on lane");
        assert_eq!(preview.target_track, track);
        assert_eq!(preview.end_frame - preview.start_frame, 4_800);
        assert_eq!(
            drag.destination.get(),
            Some(BrowserDropDestination::ExistingTrack {
                track,
                start: preview.start_frame,
            })
        );
        preview.start_frame
    });
    assert!(visual.debug_bounds("clip-drop-preview").is_some());
    crate::editor::clip_boundary::tests::assert_open_upper_right(
        &mut visual,
        "clip-drop-preview",
        "clip-drop-preview-trailing",
        None,
    );

    // Moving back out of the lane restores the cursor ghost and clears the target.
    visual.simulate_mouse_move(outside, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let drag = view
            .active_browser_audio_drag
            .as_ref()
            .expect("drag remains active");
        assert!(!drag.lane_hovered.get());
        assert!(drag.preview.get().is_none());
    });
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.last_enqueued_import_to_track,
            Some((path.clone(), track, expected, true)),
            "release imports at the previewed start with overwrite semantics"
        );
    });
    visual.run_until_parked();
}

#[gpui::test]
fn dropping_browser_audio_on_pinned_main_cancels_import_and_track_creation(
    cx: &mut TestAppContext,
) {
    let (handle, editor, _track, _directory, _path) = browser_drag_fixture(cx);
    let track_count = editor.read_with(cx, |view, _| {
        view.state.as_ref().unwrap().project.tracks().len()
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let source = visual
        .debug_bounds("browser-entry-0")
        .expect("sample browser entry");
    let main = visual
        .debug_bounds("pinned-main-track")
        .expect("pinned main row");
    let down = point(
        source.origin.x + source.size.width / 2.0,
        source.origin.y + source.size.height / 2.0,
    );
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(down.x + px(12.0), down.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    assert!(cx.update(|app| app.has_active_drag()));

    let target = point(main.origin.x + px(20.0), main.center().y);
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let drag = view
            .active_browser_audio_drag
            .as_ref()
            .expect("browser drag remains captured");
        assert!(drag.destination.get().is_none());
        assert!(drag.preview.get().is_none());
    });
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.state.as_ref().unwrap().project.tracks().len(),
            track_count
        );
        assert!(view.active_browser_audio_drag.is_none());
        assert!(view.last_enqueued_import_to_track.is_none());
        assert!(view.last_enqueued_import_to_new_track.is_none());
    });
}

#[gpui::test]
fn browser_audio_drag_uses_command_override_live_and_at_mouse_up(cx: &mut TestAppContext) {
    let (handle, editor, track, _directory, path) = browser_drag_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let source = visual.debug_bounds("browser-entry-0").expect("wav row");
    let lane_selector = Box::leak(format!("arrangement-lane-{}", track.value()).into_boxed_str());
    let lane = visual.debug_bounds(lane_selector).expect("target lane");
    let command = Modifiers {
        platform: true,
        ..Modifiers::default()
    };
    let down = point(
        source.origin.x + source.size.width / 2.0,
        source.origin.y + source.size.height / 2.0,
    );
    visual.simulate_mouse_down(down, MouseButton::Left, command);
    visual.simulate_mouse_move(point(down.x + px(12.0), down.y), MouseButton::Left, command);
    visual.run_until_parked();
    assert!(cx.update(|app| app.has_active_drag()));
    let target = point(
        lane.origin.x + lane.size.width * 0.573,
        lane.origin.y + lane.size.height / 2.0,
    );
    visual.simulate_mouse_move(target, MouseButton::Left, command);
    visual.run_until_parked();
    for _ in 0..8 {
        if editor.read_with(cx, |view, _| {
            view.active_browser_audio_drag
                .as_ref()
                .is_some_and(|drag| drag.preview.get().is_some())
        }) {
            break;
        }
        visual.run_until_parked();
    }
    let pointer_start = editor.read_with(cx, |view, _| {
        let drag = view
            .active_browser_audio_drag
            .as_ref()
            .expect("active browser drag");
        assert!(drag.snap_modifier.get());
        let painted_lane = view.drag_lanes.get(&track).expect("painted drop lane");
        let fraction = ((f64::from(target.x) - f64::from(painted_lane.left))
            / f64::from(painted_lane.width))
        .clamp(0.0, 1.0);
        let expected_pointer_start = (fraction * drag.viewport_length as f64).floor() as u64;
        assert_eq!(
            drag.destination.get(),
            Some(BrowserDropDestination::ExistingTrack {
                track,
                start: expected_pointer_start,
            }),
            "held command bypasses adaptive snapping from initial drag through destination"
        );
        expected_pointer_start
    });
    visual.simulate_modifiers_change(Modifiers::default());
    visual.run_until_parked();
    let snapped_start = editor.read_with(cx, |view, _| {
        let drag = view
            .active_browser_audio_drag
            .as_ref()
            .expect("active browser drag");
        assert!(!drag.snap_modifier.get());
        match drag.destination.get() {
            Some(BrowserDropDestination::ExistingTrack { start, .. }) => start,
            _ => panic!("browser drag remains over target lane"),
        }
    });
    assert_ne!(snapped_start, pointer_start);
    visual.simulate_mouse_up(target, MouseButton::Left, command);
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.last_enqueued_import_to_track,
            Some((path.clone(), track, pointer_start, true)),
            "mouse-up modifiers are authoritative even without a final pointer move"
        );
    });
    visual.run_until_parked();
}

#[gpui::test]
fn browser_sample_drop_between_tracks_shows_only_insertion_line_and_keeps_snapped_start(
    cx: &mut TestAppContext,
) {
    let (handle, editor, tracks, _directory, path) =
        browser_drag_fixture_with_tracks(cx, &["first", "second"]);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let source = visual
        .debug_bounds("browser-entry-0")
        .expect("rendered wav browser row");
    let first_selector =
        Box::leak(format!("arrangement-lane-{}", tracks[0].value()).into_boxed_str());
    let second_selector =
        Box::leak(format!("arrangement-lane-{}", tracks[1].value()).into_boxed_str());
    let first = visual
        .debug_bounds(first_selector)
        .expect("first track lane");
    let second = visual
        .debug_bounds(second_selector)
        .expect("second track lane");
    let body = visual
        .debug_bounds("arrangement-content")
        .expect("arrangement body");
    let down = point(
        source.origin.x + source.size.width / 2.0,
        source.origin.y + source.size.height / 2.0,
    );
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(down.x + px(12.0), down.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    let target = point(
        first.origin.x + first.size.width * 0.73,
        first.origin.y + first.size.height,
    );
    assert_eq!(first.origin.x, body.origin.x);
    assert_eq!(
        first.size.width,
        body.size.width - editor.read_with(cx, |view, _| px(view.track_header_width()))
    );
    let pointer_start = (((f32::from(target.x) - f32::from(first.origin.x))
        / f32::from(first.size.width))
        * 384_000.0)
        .floor() as u64;
    let expected_pointer_start = editor.read_with(cx, |view, _| {
        view.snap_grid
            .snap_frame(
                view.state.as_ref().expect("project").project.as_ref(),
                pointer_start,
                view.visible_viewport(384_000),
                f32::from(first.size.width),
            )
            .expect("adaptive insertion start")
    });
    assert!((f32::from(target.y) - f32::from(second.origin.y)).abs() <= 1.0);
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let expected = editor.read_with(cx, |view, _| {
        let drag = view
            .active_browser_audio_drag
            .as_ref()
            .expect("browser drag active");
        assert!(drag.preview.get().is_none());
        let Some(BrowserDropDestination::NewTrack {
            before_track,
            start,
        }) = drag.destination.get()
        else {
            panic!("gap should target a newly inserted track");
        };
        assert!(drag.lane_hovered.get());
        assert_eq!(before_track, Some(tracks[1]));
        assert_eq!(
            start, expected_pointer_start,
            "drop start follows the painted lane's pointer position"
        );
        assert!(drag.insertion_line_y.get().is_some());
        start
    });
    let line = visual
        .debug_bounds("track-insert-preview")
        .expect("full-width insertion line");
    assert_eq!(line.size.width, body.size.width);
    assert!(visual.debug_bounds("clip-drop-preview").is_none());
    assert!(
        visual.debug_bounds("append-track-drop-highlight").is_none(),
        "between-track insertion must not highlight the rows below"
    );
    let ghost = visual
        .debug_bounds("browser-audio-floating-preview-hidden")
        .expect("drag ghost node remains mounted but collapsed");
    assert_eq!(ghost.size.width, px(0.0));
    assert_eq!(ghost.size.height, px(0.0));

    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.last_enqueued_import_to_new_track,
            Some((path.clone(), Some(tracks[1]), expected)),
            "release uses the same stable anchor and snapped start as the preview"
        );
    });
    visual.run_until_parked();
}

#[gpui::test]
fn browser_sample_drop_below_last_track_appends_with_line_and_locked_ghost(
    cx: &mut TestAppContext,
) {
    let (handle, editor, track, _directory, path) = browser_drag_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let source = visual
        .debug_bounds("browser-entry-0")
        .expect("rendered wav browser row");
    let lane_selector = Box::leak(format!("arrangement-lane-{}", track.value()).into_boxed_str());
    let lane = visual.debug_bounds(lane_selector).expect("last track lane");
    let body = visual
        .debug_bounds("arrangement-content")
        .expect("arrangement body");
    assert!(
        body.size.height > lane.size.height,
        "Arrangement has blank space below last row: body={:?}, lane={:?}",
        body,
        lane
    );
    let down = point(
        source.origin.x + source.size.width / 2.0,
        source.origin.y + source.size.height / 2.0,
    );
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(down.x + px(12.0), down.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    let arrangement = editor.read_with(cx, |view, _| {
        view.arrangement_bounds.expect("arrangement bounds")
    });
    let target = point(
        lane.origin.x + lane.size.width * 0.62,
        px(arrangement.top + arrangement.height - 8.0),
    );
    assert!(
        editor.read_with(cx, |view, _| view
            .painted_track_insertion_target(f32::from(target.x), f32::from(target.y))
            .is_some()),
        "last painted track should expose the blank area below it: target={target:?}, body={body:?}, lane={lane:?}, bounds={:?}/{}, lane_epoch={:?}/{}, lanes={:?}",
        editor.read_with(cx, |view, _| view.arrangement_bounds),
        editor.read_with(cx, |view, _| view.arrangement_bounds_epoch),
        editor.read_with(cx, |view, _| view.drag_lane_epochs.get(&track).copied()),
        editor.read_with(cx, |view, _| view.drag_layout_generation),
        editor.read_with(cx, |view, _| view.drag_lanes.clone()),
    );
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let start = editor.read_with(cx, |view, _| {
        let drag = view
            .active_browser_audio_drag
            .as_ref()
            .expect("browser drag active");
        assert!(drag.lane_hovered.get());
        assert!(drag.preview.get().is_none());
        let Some(BrowserDropDestination::NewTrack {
            before_track: None,
            start,
        }) = drag.destination.get()
        else {
            panic!("blank area under the final track should append");
        };
        assert!(view.browser_audio_drag_is_current(drag));
        assert!(drag.lane_hovered.get());
        start
    });
    let line = visual
        .debug_bounds("track-insert-preview")
        .expect("append insertion line");
    assert_eq!(line.size.width, body.size.width);
    editor.read_with(cx, |view, _| {
        let drag = view
            .active_browser_audio_drag
            .as_ref()
            .expect("browser drag active");
        assert!(matches!(
            drag.destination.get(),
            Some(BrowserDropDestination::NewTrack {
                before_track: None,
                ..
            })
        ));
        assert!(view.browser_audio_drag_is_current(drag));
        assert!(
            view.active_browser_audio_drag
                .as_ref()
                .filter(|drag| view.browser_audio_drag_is_current(drag))
                .is_some_and(|drag| matches!(
                    drag.destination.get(),
                    Some(BrowserDropDestination::NewTrack {
                        before_track: None,
                        ..
                    })
                ))
        );
        assert!(
            view.state()
                .is_some_and(|state| !state.project.tracks().is_empty())
        );
        let last = view.state().unwrap().project.tracks().last().unwrap().id();
        assert_eq!(
            view.drag_lane_epochs.get(&last),
            Some(&view.drag_layout_generation)
        );
        let lane = view.drag_lanes[&last];
        let bounds = view.arrangement_bounds.unwrap();
        assert!(
            lane.top + lane.height < bounds.top + bounds.height,
            "lane={lane:?} body={bounds:?}"
        );
        assert_eq!(drag.insertion_line_y.get(), Some(lane.top + lane.height));
    });
    assert!(visual.debug_bounds("append-track-drop-highlight").is_none());
    let line = visual
        .debug_bounds("track-insert-preview")
        .expect("bottom insertion line");
    assert!((line.origin.y - (lane.origin.y + lane.size.height - px(1.0))).abs() <= px(1.0));
    let ghost = visual
        .debug_bounds("browser-audio-append-preview")
        .expect("blank-space append locks the sample ghost to the new row");
    assert!(ghost.size.width > px(3.0));
    let (expected_left, expected_width, expected_top) = editor.read_with(cx, |view, _| {
        let drag = view
            .active_browser_audio_drag
            .as_ref()
            .expect("active drag");
        let metadata = drag
            .metadata
            .borrow()
            .clone()
            .expect("prepared sample duration");
        let viewport = view.visible_viewport(384_000);
        let timeline_width = body.size.width - px(view.track_header_width());
        let left = body.origin.x
            + px(
                (start.saturating_sub(viewport.start) as f64 / viewport.length as f64
                    * f64::from(timeline_width)) as f32,
            );
        let width = px((metadata.playback_frames as f64 / viewport.length as f64
            * f64::from(timeline_width)) as f32);
        (left, width, lane.origin.y + lane.size.height)
    });
    assert!((ghost.origin.x - expected_left).abs() <= px(1.0));
    assert!((ghost.size.width - expected_width).abs() <= px(1.0));
    assert!((ghost.origin.y - expected_top).abs() <= px(1.0));
    let body_after = visual
        .debug_bounds("arrangement-content")
        .expect("Arrangement body remains mounted");
    assert_eq!(
        body_after.origin, body.origin,
        "drag must not shift the workspace"
    );
    assert_eq!(
        body_after.size, body.size,
        "drag must not resize the workspace"
    );
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.last_enqueued_import_to_new_track,
            Some((path.clone(), None, start))
        );
    });
    visual.run_until_parked();
}

#[gpui::test]
fn browser_sample_drop_in_empty_arrangement_creates_bottom_track(cx: &mut TestAppContext) {
    let (handle, editor, tracks, _directory, path) = browser_drag_fixture_with_tracks(cx, &[]);
    assert!(tracks.is_empty());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let source = visual
        .debug_bounds("browser-entry-0")
        .expect("rendered wav browser row");
    let body = visual
        .debug_bounds("arrangement-content")
        .expect("empty arrangement body");
    let down = point(
        source.origin.x + source.size.width / 2.0,
        source.origin.y + source.size.height / 2.0,
    );
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(down.x + px(12.0), down.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    let target = point(
        body.origin.x + body.size.width * 0.4,
        body.origin.y + body.size.height / 2.0,
    );
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let start = editor.read_with(cx, |view, _| {
        let drag = view
            .active_browser_audio_drag
            .as_ref()
            .expect("browser drag active");
        assert!(drag.lane_hovered.get());
        assert!(drag.preview.get().is_none());
        let Some(BrowserDropDestination::NewTrack {
            before_track: None,
            start,
        }) = drag.destination.get()
        else {
            panic!("empty Arrangement should target a bottom track");
        };
        start
    });
    assert!(
        visual.debug_bounds("track-insert-preview").is_none(),
        "empty Arrangement shows the clip without a cursor-following line"
    );
    assert!(visual.debug_bounds("empty-track-drop-highlight").is_none());
    let ghost = visual
        .debug_bounds("browser-audio-append-preview")
        .expect("empty Arrangement locks the sample ghost to the first row");
    assert!(ghost.size.width > px(3.0));
    assert!(
        (ghost.origin.x
            - body.origin.x
            - px((start as f64 / 384_000.0
                * f64::from(
                    body.size.width - editor.read_with(cx, |view, _| px(view.track_header_width()))
                )) as f32))
        .abs()
            <= px(1.0)
    );
    assert!((ghost.origin.y - body.origin.y).abs() <= px(1.0));
    // The pointer-down audition is still playing when the sample lands.
    editor.update(cx, |view, _| {
        view.browser.playback_phase = hex_audio::sample_preview::Phase::Playing;
        view.browser.playback_output = Some((1, 1));
        view.browser.playback_path = Some(path.clone());
    });
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.last_enqueued_import_to_new_track,
            Some((path.clone(), None, start))
        );
        assert!(view.browser.playback_output.is_none(), "drop stops preview");
        assert_eq!(
            view.browser.playback_phase,
            hex_audio::sample_preview::Phase::Idle
        );
    });
    visual.run_until_parked();
}

#[gpui::test]
fn browser_sample_gap_target_uses_only_currently_painted_rows_after_scroll(
    cx: &mut TestAppContext,
) {
    let names = (0..24)
        .map(|index| format!("track {index}"))
        .collect::<Vec<_>>();
    let name_refs = names.iter().map(String::as_str).collect::<Vec<_>>();
    let (handle, editor, tracks, _directory, _) = browser_drag_fixture_with_tracks(cx, &name_refs);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let arrangement = visual
        .debug_bounds("arrangement-content")
        .expect("painted arrangement body");
    let pan_start = point(
        arrangement.origin.x + px(500.0),
        arrangement.origin.y + px(250.0),
    );
    visual.simulate_mouse_down(pan_start, MouseButton::Middle, Modifiers::default());
    visual.simulate_mouse_move(
        point(pan_start.x, pan_start.y - px(1_100.0)),
        MouseButton::Middle,
        Modifiers::default(),
    );
    visual.simulate_mouse_up(
        point(pan_start.x, pan_start.y - px(1_100.0)),
        MouseButton::Middle,
        Modifiers::default(),
    );
    visual.run_until_parked();
    let (first_track, second_track, target) = editor.update(cx, |view, _| {
        let generation = view.drag_layout_generation;
        let tracks = &view.state.as_ref().expect("project").project.tracks();
        let pair = tracks.windows(2).find(|pair| {
            view.drag_lane_epochs.get(&pair[0].id()) == Some(&generation)
                && view.drag_lane_epochs.get(&pair[1].id()) == Some(&generation)
        });
        let pair = pair.expect("at least two adjacent rows are painted");
        let first_lane = view.drag_lanes[&pair[0].id()];
        let second_lane = view.drag_lanes[&pair[1].id()];
        let pointer_x = first_lane.left + first_lane.width * 0.62;
        let pointer_y = first_lane.top + first_lane.height;
        assert!(
            view.drag_lane_epochs[&tracks[0].id()] < generation,
            "first row should be offscreen after scroll; epochs={:?}, generation={generation}, lanes={:?}, arrangement={:?}",
            view.drag_lane_epochs,
            view.drag_lanes,
            view.arrangement_bounds
        );
        view.drag_lanes.insert(tracks[0].id(), second_lane);
        let insertion = view
            .painted_track_insertion_target(pointer_x, pointer_y)
            .expect("visible gap remains a valid insertion target");
        assert_eq!(insertion.0, Some(pair[1].id()));
        (
            pair[0].id(),
            pair[1].id(),
            point(px(pointer_x), px(pointer_y)),
        )
    });
    assert_ne!(first_track, tracks[0]);

    let source = visual
        .debug_bounds("browser-entry-0")
        .expect("rendered wav browser row");
    let down = point(
        source.origin.x + source.size.width / 2.0,
        source.origin.y + source.size.height / 2.0,
    );
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(down.x + px(12.0), down.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let drag = view
            .active_browser_audio_drag
            .as_ref()
            .expect("browser drag active");
        assert!(matches!(
            drag.destination.get(),
            Some(BrowserDropDestination::NewTrack {
                before_track: Some(track),
                ..
            }) if track == second_track
        ));
        assert!(drag.insertion_line_y.get().is_some());
    });
    assert!(visual.debug_bounds("track-insert-preview").is_some());
}

#[gpui::test]
fn arrangement_wheel_zoom_is_pointer_anchored_and_does_not_escape_its_timeline(
    cx: &mut TestAppContext,
) {
    let names = (0..24)
        .map(|index| format!("track {index}"))
        .collect::<Vec<_>>();
    let name_refs = names.iter().map(String::as_str).collect::<Vec<_>>();
    let (handle, editor, _, _directory, _) = browser_drag_fixture_with_tracks(cx, &name_refs);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();

    let ruler = visual
        .debug_bounds("timeline-ruler")
        .expect("painted arrangement ruler");
    let body = visual
        .debug_bounds("arrangement-content")
        .expect("painted arrangement body");
    let grid = visual
        .debug_bounds("arrangement-grid")
        .expect("painted Arrangement grid");
    let first_grid_line = visual
        .debug_bounds("arrangement-grid-line-0")
        .expect("grid starts at the visible timeline origin");
    assert_eq!(grid.origin.x, body.origin.x);
    assert_eq!(
        grid.size.width + editor.read_with(cx, |view, _| px(view.track_header_width())),
        body.size.width
    );
    assert_eq!(first_grid_line.origin.x, ruler.origin.x);
    let pointer = point(
        ruler.origin.x + ruler.size.width * 0.37,
        ruler.origin.y + ruler.size.height / 2.0,
    );
    let before = editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().expect("project");
        view.visible_viewport(state.timeline_frames)
    });
    visual.simulate_event(gpui::ScrollWheelEvent {
        position: pointer,
        delta: gpui::ScrollDelta::Pixels(point(px(0.0), px(80.0))),
        ..Default::default()
    });
    visual.run_until_parked();
    let after = editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().expect("project");
        view.visible_viewport(state.timeline_frames)
    });
    let anchor = super::super::seek::pointer_to_frame(
        f32::from(pointer.x),
        f32::from(ruler.origin.x),
        f32::from(ruler.size.width),
        before,
    )
    .expect("anchor inside ruler");
    assert_eq!(after, before.zoom_at(384_000, anchor, true));
    let old_fraction = (anchor - before.start) as f64 / before.length as f64;
    let new_fraction = (anchor - after.start) as f64 / after.length as f64;
    assert!((old_fraction - new_fraction).abs() < 0.002);

    visual.simulate_event(gpui::ScrollWheelEvent {
        position: pointer,
        direction_inverted_from_device: true,
        delta: gpui::ScrollDelta::Pixels(point(px(0.0), px(80.0))),
        ..Default::default()
    });
    visual.run_until_parked();
    let natural_down = editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().expect("project");
        view.visible_viewport(state.timeline_frames)
    });
    assert_eq!(
        natural_down.length, before.length,
        "physical wheel down zooms out"
    );
    visual.simulate_event(gpui::ScrollWheelEvent {
        position: pointer,
        direction_inverted_from_device: true,
        delta: gpui::ScrollDelta::Pixels(point(px(0.0), px(-80.0))),
        ..Default::default()
    });
    visual.run_until_parked();
    let natural_up = editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().expect("project");
        view.visible_viewport(state.timeline_frames)
    });
    assert_eq!(
        natural_up, after,
        "physical wheel up zooms in with inversion enabled"
    );

    visual.simulate_event(gpui::ScrollWheelEvent {
        position: pointer,
        delta: gpui::ScrollDelta::Lines(point(0.0, -1.0)),
        ..Default::default()
    });
    visual.run_until_parked();
    let line_out = editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().expect("project");
        view.visible_viewport(state.timeline_frames)
    });
    assert_eq!(
        line_out.length, 384_000,
        "one physical wheel-down line zooms out once"
    );
    visual.simulate_event(gpui::ScrollWheelEvent {
        position: pointer,
        delta: gpui::ScrollDelta::Lines(point(0.0, 1.0)),
        ..Default::default()
    });
    visual.run_until_parked();
    let line_in = editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().expect("project");
        view.visible_viewport(state.timeline_frames)
    });
    assert_eq!(
        line_in.length, after.length,
        "one reverse wheel line zooms in once"
    );
    visual.simulate_event(gpui::ScrollWheelEvent {
        position: pointer,
        delta: gpui::ScrollDelta::Pixels(point(px(160.0), px(0.0))),
        ..Default::default()
    });
    visual.run_until_parked();
    let horizontal_only = editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().expect("project");
        view.visible_viewport(state.timeline_frames)
    });
    assert_eq!(
        horizontal_only, line_in,
        "horizontal-only wheel input does not zoom"
    );

    let (list_before, scroll_before) = editor.read_with(cx, |view, _| {
        (
            view.track_scroll.logical_scroll_top_index(),
            view.track_scroll.offset(),
        )
    });
    visual.simulate_event(gpui::ScrollWheelEvent {
        position: point(body.origin.x + px(100.0), body.origin.y + px(80.0)),
        delta: gpui::ScrollDelta::Pixels(point(px(0.0), px(-12.0))),
        ..Default::default()
    });
    visual.run_until_parked();
    let (list_after, scroll_after) = editor.read_with(cx, |view, _| {
        (
            view.track_scroll.logical_scroll_top_index(),
            view.track_scroll.offset(),
        )
    });
    assert_eq!(
        list_after, list_before,
        "small timeline wheel does not scroll track rows"
    );
    assert_eq!(
        scroll_after, scroll_before,
        "partial wheel deltas are consumed"
    );

    editor.update(cx, |view, _| {
        view.viewport = Some(super::super::viewport::Viewport {
            start: 0,
            length: 1,
        });
    });
    visual.run_until_parked();
    let at_zoom_limit = editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().expect("project");
        (
            view.visible_viewport(state.timeline_frames),
            view.track_scroll.offset(),
        )
    });
    visual.simulate_event(gpui::ScrollWheelEvent {
        position: point(body.origin.x + px(100.0), body.origin.y + px(80.0)),
        delta: gpui::ScrollDelta::Pixels(point(px(0.0), px(80.0))),
        ..Default::default()
    });
    visual.run_until_parked();
    let after_limit_wheel = editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().expect("project");
        (
            view.visible_viewport(state.timeline_frames),
            view.track_scroll.offset(),
        )
    });
    assert_eq!(after_limit_wheel, at_zoom_limit);

    let browser_handle = visual
        .debug_bounds("browser-resize-handle")
        .expect("visible browser");
    let browser_pointer = point(
        browser_handle.origin.x - px(60.0),
        browser_handle.origin.y + browser_handle.size.height / 2.0,
    );
    let before_browser_wheel = editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().expect("project");
        view.visible_viewport(state.timeline_frames)
    });
    visual.simulate_event(gpui::ScrollWheelEvent {
        position: browser_pointer,
        delta: gpui::ScrollDelta::Pixels(point(px(0.0), px(-160.0))),
        ..Default::default()
    });
    visual.run_until_parked();
    let after_browser_wheel = editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().expect("project");
        view.visible_viewport(state.timeline_frames)
    });
    assert_eq!(after_browser_wheel, before_browser_wheel);
}

#[test]
fn physical_wheel_delta_normalizes_pixels_and_lines_without_mutating_raw_delta() {
    let cases = [
        (
            gpui::ScrollDelta::Pixels(point(px(12.0), px(24.0))),
            false,
            (12.0, 24.0),
        ),
        (
            gpui::ScrollDelta::Pixels(point(px(12.0), px(24.0))),
            true,
            (-12.0, -24.0),
        ),
        (
            gpui::ScrollDelta::Lines(point(2.0, -3.0)),
            false,
            (2.0, -3.0),
        ),
        (
            gpui::ScrollDelta::Lines(point(2.0, -3.0)),
            true,
            (-2.0, 3.0),
        ),
    ];
    for (raw, inverted, (expected_x, expected_y)) in cases {
        let event = gpui::ScrollWheelEvent {
            delta: raw,
            direction_inverted_from_device: inverted,
            ..Default::default()
        };
        match event.physical_delta() {
            gpui::ScrollDelta::Pixels(delta) => {
                assert_eq!(f32::from(delta.x), expected_x);
                assert_eq!(f32::from(delta.y), expected_y);
            }
            gpui::ScrollDelta::Lines(delta) => {
                assert_eq!(delta.x, expected_x);
                assert_eq!(delta.y, expected_y);
            }
        }
        match (raw, event.delta) {
            (gpui::ScrollDelta::Pixels(before), gpui::ScrollDelta::Pixels(after)) => {
                assert_eq!(before, after);
            }
            (gpui::ScrollDelta::Lines(before), gpui::ScrollDelta::Lines(after)) => {
                assert_eq!(before.x, after.x);
                assert_eq!(before.y, after.y);
            }
            _ => panic!("raw scroll delta variant changed"),
        }
    }
}

#[gpui::test]
fn empty_arrangement_keeps_the_grid_aligned_with_the_ruler(cx: &mut TestAppContext) {
    let (handle, editor, tracks, _directory, _) = browser_drag_fixture_with_tracks(cx, &[]);
    assert!(tracks.is_empty());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let body = visual
        .debug_bounds("arrangement-content")
        .expect("empty Arrangement body");
    let ruler = visual
        .debug_bounds("timeline-ruler")
        .expect("empty Arrangement ruler");
    let grid = visual
        .debug_bounds("arrangement-grid")
        .expect("empty Arrangement grid");
    let line = visual
        .debug_bounds("arrangement-grid-line-0")
        .expect("empty Arrangement first grid line");
    assert!(
        visual.debug_bounds("arrangement-grid-band-0").is_some(),
        "four-adaptive-unit shading spans the empty Arrangement timeline"
    );
    assert_eq!(grid.origin.x, body.origin.x);
    assert_eq!(
        grid.size.width + editor.read_with(cx, |view, _| px(view.track_header_width())),
        body.size.width
    );
    assert_eq!(line.origin.x, ruler.origin.x);
}

#[gpui::test]
fn middle_drag_pans_both_arrangement_axes_until_release(cx: &mut TestAppContext) {
    let names = (0..32)
        .map(|index| format!("track {index}"))
        .collect::<Vec<_>>();
    let name_refs = names.iter().map(String::as_str).collect::<Vec<_>>();
    let (handle, editor, _, _directory, _) = browser_drag_fixture_with_tracks(cx, &name_refs);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    editor.update(cx, |view, _| {
        view.viewport = Some(super::super::viewport::Viewport {
            start: 96_000,
            length: 192_000,
        });
    });
    visual.run_until_parked();
    let body = visual
        .debug_bounds("arrangement-content")
        .expect("painted arrangement body");
    let start = point(body.origin.x + px(400.0), body.origin.y + px(100.0));
    let initial = editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().expect("project");
        view.visible_viewport(state.timeline_frames)
    });
    visual.simulate_mouse_down(start, MouseButton::Middle, Modifiers::default());
    visual.simulate_mouse_move(
        point(start.x - px(90.0), start.y - px(180.0)),
        MouseButton::Middle,
        Modifiers::default(),
    );
    visual.run_until_parked();
    let (panned, active, scroll_offset, scroll_max) = editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().expect("project");
        (
            view.visible_viewport(state.timeline_frames),
            view.arrangement_pan.is_some(),
            view.track_scroll.offset(),
            view.track_scroll.max_offset(),
        )
    });
    let timeline_width =
        f32::from(body.size.width - editor.read_with(cx, |view, _| px(view.track_header_width())));
    let expected_delta = (90.0 * initial.length as f64 / f64::from(timeline_width)).round() as i128;
    assert_eq!(
        panned,
        initial.pan_by_frames(384_000, expected_delta),
        "the held move outside Arrangement pans by the measured timeline width"
    );
    assert!(
        scroll_offset.y < px(0.0),
        "vertical drag updates the list scroll handle; max={scroll_max:?}"
    );
    assert!(active);

    let outside = point(start.x - px(90.0), px(-18.0));
    visual.simulate_mouse_move(
        point(outside.x, outside.y - px(10_000.0)),
        MouseButton::Middle,
        Modifiers::default(),
    );
    visual.run_until_parked();
    let bottom_offset = editor.read_with(cx, |view, _| {
        let handle = view.track_scroll.clone();
        (handle.offset().y, handle.max_offset().height)
    });
    assert_eq!(
        bottom_offset.0, -bottom_offset.1,
        "vertical pan clamps at the last row"
    );
    visual.simulate_mouse_move(outside, MouseButton::Middle, Modifiers::default());
    visual.run_until_parked();
    let top_offset = editor.read_with(cx, |view, _| view.track_scroll.offset().y);
    assert_eq!(top_offset, px(0.0), "vertical pan clamps at the first row");
    visual.simulate_mouse_up(outside, MouseButton::Middle, Modifiers::default());
    visual.run_until_parked();
    let after_release = editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().expect("project");
        (
            view.visible_viewport(state.timeline_frames),
            view.track_scroll.offset(),
            view.arrangement_pan.is_some(),
        )
    });
    assert_eq!(
        after_release,
        (panned, point(scroll_offset.x, top_offset), false)
    );
    visual.simulate_mouse_move(
        point(outside.x - px(30.0), outside.y - px(30.0)),
        None,
        Modifiers::default(),
    );
    visual.run_until_parked();
    let after_unpressed_move = editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().expect("project");
        (
            view.visible_viewport(state.timeline_frames),
            view.track_scroll.offset(),
        )
    });
    assert_eq!(
        after_unpressed_move,
        (panned, point(scroll_offset.x, top_offset))
    );
}

#[gpui::test]
fn browser_audio_drag_release_outside_arrangement_cancels_import(cx: &mut TestAppContext) {
    let (handle, editor, _, _directory, _) = browser_drag_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let source = visual
        .debug_bounds("browser-entry-0")
        .expect("rendered wav browser row");
    let down = point(
        source.origin.x + source.size.width / 2.0,
        source.origin.y + source.size.height / 2.0,
    );
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(down.x + px(12.0), down.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    let outside = point(source.origin.x + px(24.0), source.origin.y + px(8.0));
    visual.simulate_mouse_move(outside, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    visual.simulate_mouse_up(outside, MouseButton::Left, Modifiers::default());
    editor.read_with(cx, |view, _| {
        assert!(view.last_enqueued_import_to_track.is_none());
        assert!(view.active_browser_audio_drag.is_none());
    });
    visual.run_until_parked();
}

#[gpui::test]
fn browser_audio_drag_rejects_changed_snap_or_viewport(cx: &mut TestAppContext) {
    let (_handle, editor, _, _directory, path) = browser_drag_fixture(cx);
    editor.update(cx, |view, _| {
        let state = view.state.as_ref().expect("project state");
        let viewport = view.visible_viewport(state.timeline_frames);
        view.snap_grid = super::super::musical_time::SnapGrid::Quarter;
        let payload = BrowserAudioDrag {
            project_generation: view.project_generation,
            revision: state.revision,
            browser_generation: Some(view.browser.scan_generation()),
            sample_rate: state.project.sample_rate(),
            path,
            label: "loop.wav".into(),
            viewport_start: viewport.start,
            viewport_length: viewport.length,
            scroll: Default::default(),
            snap_grid: view.snap_grid,
            snap_modifier: Rc::new(std::cell::Cell::new(false)),
            copy_modifier: Rc::new(std::cell::Cell::new(false)),
            metadata: Rc::new(RefCell::new(None)),
            metadata_error: Rc::new(RefCell::new(None)),
            preview: Rc::new(std::cell::Cell::new(None)),
            destination: Rc::new(std::cell::Cell::new(None)),
            insertion_line_y: Rc::new(std::cell::Cell::new(None)),
            lane_hovered: Rc::new(std::cell::Cell::new(false)),
            last_pointer: Rc::new(std::cell::Cell::new(None)),
        };
        view.begin_browser_audio_drag(&payload);
        assert!(view.browser_audio_drag_is_current(&payload));
        view.snap_grid = super::super::musical_time::SnapGrid::Off;
        assert!(!view.browser_audio_drag_is_current(&payload));
        view.snap_grid = payload.snap_grid;
        view.viewport = Some(super::super::viewport::Viewport {
            start: 0,
            length: viewport.length / 2,
        });
        assert!(!view.browser_audio_drag_is_current(&payload));
    });
}

#[gpui::test]
fn clip_and_sample_drags_clear_unavailable_targets_and_recover_from_invalid_pointer(
    cx: &mut TestAppContext,
) {
    use hex_session::service::AudioPreview;
    use hex_session::{CachedPeak as Peak, CachedWaveform as Waveform};
    use std::cell::Cell;

    let (handle, editor, track, _directory, path) = browser_drag_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let (return_id, browser_drag, clip_drag) = editor.update(cx, |view, cx| {
        let current = Arc::clone(view.state.as_ref().expect("project state"));
        let mut project = Editor::new((*current.project).clone(), 8).expect("editor");
        let Created::ReturnTrack(return_id) = project
            .execute(Command::AddReturnTrack {
                name: "send bus".into(),
            })
            .expect("return track")
        else {
            panic!("return track expected")
        };
        let Created::Clip(clip_id) = project
            .execute(Command::PlaceMidiClip {
                track,
                start: hex_project::BeatTime::ZERO,
                length: hex_project::BeatTime::from_quarters(4).expect("clip length"),
            })
            .expect("midi clip")
        else {
            panic!("midi clip expected")
        };
        let clip = project.project().midi_clip(clip_id).expect("clip");
        view.state = Some(Arc::new(ViewState {
            canonical_audio_source: None,
            comparison_audio: None,
            project: Arc::new(project.project().clone()),
            waveforms: current.waveforms.clone(),
            source_overviews: current.source_overviews.clone(),
            audio_clip_waveforms: std::collections::BTreeMap::new(),
            midi_preview: current.midi_preview.clone(),
            gain_preview: Ok(Arc::new(hex_engine::GainAutomationPreview::default())),
            timeline_frames: current.timeline_frames,
            can_undo: current.can_undo,
            can_redo: current.can_redo,
            revision: current.revision.saturating_add(1),
            unsaved: current.unsaved,
            document_path: current.document_path.clone(),
        }));
        let viewport = view.visible_viewport(current.timeline_frames);
        let browser_drag = BrowserAudioDrag {
            project_generation: view.project_generation,
            revision: current.revision.saturating_add(1),
            browser_generation: Some(view.browser.scan_generation()),
            sample_rate: current.project.sample_rate(),
            path,
            label: "loop.wav".into(),
            viewport_start: viewport.start,
            viewport_length: viewport.length,
            scroll: Default::default(),
            snap_grid: view.snap_grid,
            snap_modifier: Rc::new(Cell::new(false)),
            copy_modifier: Rc::new(Cell::new(false)),
            metadata: Rc::new(RefCell::new(Some(Rc::new(AudioPreview {
                playback_frames: 480,
                sample_rate: current.project.sample_rate(),
                waveform: Arc::new(
                    Waveform::from_peaks(
                        480,
                        vec![
                            [Peak {
                                min: -0.2,
                                max: 0.2
                            }; 2];
                            48
                        ],
                    )
                    .expect("sample waveform"),
                ),
            })))),
            metadata_error: Rc::new(RefCell::new(None)),
            preview: Rc::new(Cell::new(None)),
            destination: Rc::new(Cell::new(None)),
            insertion_line_y: Rc::new(Cell::new(None)),
            lane_hovered: Rc::new(Cell::new(false)),
            last_pointer: Rc::new(Cell::new(None)),
        };
        let clip_drag = ClipDrag {
            group: Default::default(),
            project_generation: view.project_generation,
            revision: current.revision.saturating_add(1),
            source_track: track,
            clip: clip_id,
            position: ClipPosition::Midi {
                start: clip.start(),
                length: clip.length(),
            },
            grab_offset: Rc::new(Cell::new(Some(0))),
            snap_edge: Rc::new(Cell::new(Some(super::drag::ClipSnapEdge::Leading))),
            source_span: Default::default(),
            capture: Rc::new(Cell::new(None)),
            viewport_start: viewport.start,
            viewport_length: viewport.length,
            scroll: Default::default(),
            snap_grid: view.snap_grid,
            snap_modifier: Rc::new(Cell::new(false)),
            copy_modifier: Rc::new(Cell::new(false)),
            preview: Rc::new(Cell::new(None)),
            destination: Rc::new(Cell::new(None)),
            insertion_line_y: Rc::new(Cell::new(None)),
            ghost_label: "clip".into(),
            ghost_waveform: None,
            lane_hovered: Rc::new(Cell::new(false)),
            last_pointer: Rc::new(Cell::new(None)),
        };
        cx.notify();
        (return_id, browser_drag, clip_drag)
    });
    visual.run_until_parked();
    let lane_selector = Box::leak(format!("arrangement-lane-{}", track.value()).into_boxed_str());
    let lane = visual.debug_bounds(lane_selector).expect("track lane");
    let return_selector =
        Box::leak(format!("return-track-row-{}", return_id.value()).into_boxed_str());
    let return_row = visual
        .debug_bounds(return_selector)
        .expect("pinned return row");
    let lane_point = point(lane.origin.x + px(120.0), lane.center().y);
    let return_point = point(lane_point.x, return_row.center().y);

    editor.update(cx, |view, _| {
        view.begin_browser_audio_drag(&browser_drag);
        let (preview, _) = view.browser_audio_drop_position(
            &browser_drag,
            f32::from(lane_point.x),
            f32::from(lane_point.y),
        );
        assert!(
            preview.is_some(),
            "sample should preview over a regular track"
        );
        assert!(matches!(
            browser_drag.destination.get(),
            Some(BrowserDropDestination::ExistingTrack { track: target, .. }) if target == track
        ));
        browser_drag.preview.set(preview);
        let (preview, lane_hovered) = view.browser_audio_drop_position(
            &browser_drag,
            f32::from(return_point.x),
            f32::from(return_point.y),
        );
        browser_drag.preview.set(preview);
        assert!(preview.is_none());
        assert!(!lane_hovered);
        assert!(browser_drag.destination.get().is_none());
        assert!(browser_drag.insertion_line_y.get().is_none());

        view.begin_clip_drag(&clip_drag);
        let (preview, _) = view.clip_drag_drop_position(
            &clip_drag,
            f32::from(lane_point.x),
            f32::from(lane_point.y),
        );
        assert!(
            preview.is_some(),
            "clip should preview over a regular track"
        );
        assert_eq!(
            clip_drag.destination.get(),
            Some(ClipDropDestination::ExistingTrack(track))
        );
        for (invalid, origin) in [
            (f32::NAN, None),
            (f32::INFINITY, None),
            (f32::NEG_INFINITY, None),
            (-f32::MAX, Some(f32::MAX)),
        ] {
            for target_y in [lane_point.y, lane.top()] {
                clip_drag.capture.set(None);
                let (valid, hovered) = view.clip_drag_drop_position(
                    &clip_drag,
                    f32::from(lane_point.x),
                    f32::from(target_y),
                );
                assert!(hovered);
                assert!(clip_drag.destination.get().is_some());
                if target_y == lane_point.y {
                    assert!(valid.is_some());
                } else {
                    assert!(matches!(
                        clip_drag.destination.get(),
                        Some(ClipDropDestination::NewTrack { .. })
                    ));
                    assert!(clip_drag.insertion_line_y.get().is_some());
                }
                clip_drag.capture.set(origin.and_then(|x| {
                    super::drag::PointerDown::begin(
                        super::drag::ClipPointerKey {
                            generation: clip_drag.project_generation,
                            clip: clip_drag.clip,
                        },
                        hex_widgets::RectanglePart::Body,
                        x,
                        true,
                    )
                }));
                let held = clip_drag.capture.get();
                let (rejected, hovered) =
                    view.clip_drag_drop_position(&clip_drag, invalid, f32::from(target_y));
                assert!(rejected.is_none() && !hovered);
                assert!(
                    clip_drag.destination.get().is_none(),
                    "invalid pointer must clear the previous target"
                );
                assert!(clip_drag.insertion_line_y.get().is_none());
                assert_eq!(clip_drag.capture.get(), held, "retain the finite press");
                let (recovered, hovered) = view.clip_drag_drop_position(
                    &clip_drag,
                    f32::from(lane_point.x),
                    f32::from(lane_point.y),
                );
                assert!(recovered.is_some() && hovered);
                assert_eq!(
                    clip_drag.destination.get(),
                    Some(ClipDropDestination::ExistingTrack(track))
                );
            }
        }
        clip_drag.preview.set(preview);
        let (preview, lane_hovered) = view.clip_drag_drop_position(
            &clip_drag,
            f32::from(return_point.x),
            f32::from(return_point.y),
        );
        clip_drag.preview.set(preview);
        assert!(preview.is_none());
        assert!(!lane_hovered);
        assert!(clip_drag.destination.get().is_none());
        assert!(clip_drag.insertion_line_y.get().is_none());
    });
}

#[gpui::test]
fn browser_restores_missing_user_folder_without_touching_real_preferences(cx: &mut TestAppContext) {
    use gpui::VisualTestContext;
    let directory = PreferencesTestDirectory::new();
    let preferences_path = directory.0.join("browser.json");
    let saved =
        hex_preferences::BrowserPlaces::new([directory.0.join("folder-that-is-currently-missing")])
            .expect("saved missing folder");
    hex_preferences::save(&preferences_path, &saved).expect("write isolated preferences");
    let bootstrap = cx.add_window(|_, _| Empty);
    let handle = *bootstrap;
    let editor = cx
        .update_window(handle, |_, window, cx| {
            let service = Service::new(48_000, 32, 8_000_000).expect("test service");
            window.replace_root(cx, |window, cx| {
                EditorView::new_for_test(
                    window,
                    cx,
                    service,
                    Err("offline test".into()),
                    preferences_path.clone(),
                    directory.0.join("recent-projects.json"),
                )
            })
        })
        .expect("editor window");
    let visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(view.browser.places.contains(&saved.additional()[0]));
        assert!(view.browser.preferences_writable);
    });
    assert_eq!(
        hex_preferences::load(&preferences_path).expect("preserved saved places"),
        saved
    );
}

#[gpui::test]
fn browser_preferences_coalesce_background_saves_and_preserve_corrupt_files(
    cx: &mut TestAppContext,
) {
    use gpui::VisualTestContext;
    let directory = PreferencesTestDirectory::new();
    let preferences_path = directory.0.join("browser.json");
    let bootstrap = cx.add_window(|_, _| Empty);
    let handle = *bootstrap;
    let editor = cx
        .update_window(handle, |_, window, cx| {
            let service = Service::new(48_000, 32, 8_000_000).expect("test service");
            window.replace_root(cx, |window, cx| {
                EditorView::new_for_test(
                    window,
                    cx,
                    service,
                    Err("offline test".into()),
                    preferences_path.clone(),
                    directory.0.join("recent-projects.json"),
                )
            })
        })
        .expect("editor window");
    let visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let first = directory.0.join("first-added-folder");
    let second = directory.0.join("second-added-folder");
    editor.update(cx, |view, cx| {
        view.browser.places.push(first.clone());
        view.queue_browser_places_save(cx);
        view.browser.places.push(second.clone());
        view.queue_browser_places_save(cx);
    });
    visual.run_until_parked();
    let saved = hex_preferences::load(&preferences_path).expect("coalesced save");
    assert_eq!(saved.additional(), &[first.clone(), second.clone()]);

    let corrupt = b"not valid browser preferences";
    std::fs::write(&preferences_path, corrupt).expect("replace with corrupt fixture");
    editor.update(cx, |view, cx| {
        view.browser.places.push(directory.0.join("third-folder"));
        view.queue_browser_places_save(cx);
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(
            view.browser
                .places_status
                .as_deref()
                .is_some_and(|status| status.contains("could not save"))
        );
    });
    assert_eq!(
        std::fs::read(&preferences_path).expect("corrupt file remains"),
        corrupt
    );
}

#[gpui::test]
fn browser_places_failed_save_survives_a_later_queued_success(cx: &mut TestAppContext) {
    use gpui::VisualTestContext;
    let directory = PreferencesTestDirectory::new();
    let corrupt_path = directory.0.join("corrupt.json");
    let good_path = directory.0.join("browser.json");
    let bootstrap = cx.add_window(|_, _| Empty);
    let handle = *bootstrap;
    let editor = cx
        .update_window(handle, |_, window, cx| {
            let service = Service::new(48_000, 32, 8_000_000).expect("test service");
            window.replace_root(cx, |window, cx| {
                EditorView::new_for_test(
                    window,
                    cx,
                    service,
                    Err("offline test".into()),
                    corrupt_path.clone(),
                    directory.0.join("recent-projects.json"),
                )
            })
        })
        .expect("editor window");
    let visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    // Corrupt preferences are preserved, so the first save fails.
    std::fs::write(&corrupt_path, b"not valid browser preferences").expect("corrupt fixture");
    let first = directory.0.join("first-folder");
    let second = directory.0.join("second-folder");
    editor.update(cx, |view, cx| {
        view.browser.places.push(first.clone());
        view.queue_browser_places_save(cx);
        // The queued save targets a writable file and succeeds.
        view.browser.set_test_preferences_path(good_path.clone());
        view.browser.places.push(second.clone());
        view.queue_browser_places_save(cx);
    });
    visual.run_until_parked();
    assert_eq!(
        hex_preferences::load(&good_path)
            .expect("queued save")
            .additional(),
        &[first, second]
    );
    editor.read_with(cx, |view, _| {
        let status = view.browser.places_status.as_deref().unwrap_or_default();
        assert!(
            status.contains("could not save browser places")
                && status.contains("a later save succeeded"),
            "earlier failure must stay visible after a queued success: {status:?}"
        );
    });
}

#[gpui::test]
fn browser_sample_drop_above_first_track_shows_line_and_inserts_before_it(cx: &mut TestAppContext) {
    let (handle, editor, tracks, _directory, path) =
        browser_drag_fixture_with_tracks(cx, &["first", "second"]);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let source = visual.debug_bounds("browser-entry-0").expect("sample row");
    let selector = Box::leak(format!("arrangement-lane-{}", tracks[0].value()).into_boxed_str());
    let first = visual.debug_bounds(selector).expect("first lane");
    let down = point(
        source.origin.x + source.size.width / 2.0,
        source.origin.y + source.size.height / 2.0,
    );
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(down.x + px(12.0), down.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    let target = point(
        first.origin.x + first.size.width / 2.0,
        first.origin.y + px(1.0),
    );
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("track-insert-preview").is_some());
    assert!(visual.debug_bounds("clip-drop-preview").is_none());
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    editor.read_with(cx, |view, _| {
        let (dropped_path, anchor, _) = view
            .last_enqueued_import_to_new_track
            .as_ref()
            .expect("new track import");
        assert_eq!(dropped_path, &path);
        assert_eq!(*anchor, Some(tracks[0]));
        assert!(view.last_enqueued_import_to_track.is_none());
    });
}

#[gpui::test]
fn browser_click_and_right_request_preview_without_import_search_arrows_keep_caret(
    cx: &mut TestAppContext,
) {
    cx.update(super::super::bind_editor_keys);
    let (handle, editor, track, directory, path) = browser_drag_fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    let row = visual.debug_bounds("browser-entry-0").expect("sample row");
    visual.simulate_mouse_down(row.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(row.center(), MouseButton::Left, Modifiers::default());
    editor.read_with(visual, |view, _| {
        assert_eq!(view.browser.playback_generation, 1);
        assert!(
            view.error
                .as_ref()
                .is_some_and(|error| error.contains("sample preview"))
        );
        assert!(view.pending_requests.is_empty());
        assert!(view.pending_audio_commands.is_empty());
    });
    // A real second mouse-down with a valid target track must never import.
    editor.update(visual, |view, _| view.selected_track = Some(track));
    for auto in [true, false] {
        editor.update(visual, |view, _| view.browser.auto_audition = auto);
        let (before, state) = editor.read_with(visual, |view, _| {
            (
                view.browser.playback_generation,
                view.state.clone().unwrap(),
            )
        });
        visual.simulate_event(gpui::MouseDownEvent {
            position: row.center(),
            button: MouseButton::Left,
            click_count: 2,
            first_mouse: false,
            modifiers: Modifiers::default(),
        });
        visual.simulate_mouse_up(row.center(), MouseButton::Left, Modifiers::default());
        editor.read_with(visual, |view, _| {
            assert_eq!(view.browser.playback_generation, before + u64::from(auto));
            assert!(
                view.pending_requests.is_empty(),
                "double-click cannot enqueue import"
            );
            assert!(view.last_enqueued_import_to_track.is_none());
            assert!(view.last_enqueued_import_to_new_track.is_none());
            assert!(view.pending_audio_commands.is_empty());
            assert!(Arc::ptr_eq(view.state.as_ref().unwrap(), &state));
            assert!(view.browser.results_focused);
        });
    }
    editor.update(visual, |view, _| view.browser.auto_audition = true);
    let contexts = visual
        .update_window(handle, |_, window, cx| {
            let view = editor.read(cx);
            (
                window.context_stack(),
                view.browser_key_context(window, cx),
                view.browser.results_focused,
            )
        })
        .unwrap();
    assert_eq!(contexts.1, "hex_browser", "{contexts:?}");
    assert!(contexts.2, "{contexts:?}");
    visual.simulate_keystrokes("right");
    editor.read_with(visual, |view, _| {
        assert_eq!(view.browser.playback_generation, 3, "{contexts:?}");
        assert!(view.pending_requests.is_empty());
    });
    // Clicking search restores text focus. Left/right then insert at the
    // restored caret; neither key may send an audition request.
    let search = visual.debug_bounds("browser-search").expect("search field");
    visual.simulate_mouse_down(search.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(search.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("right l o left right o");
    editor.read_with(visual, |view, cx| {
        assert_eq!(view.browser.search.read(cx).text(), "loo");
        assert_eq!(view.browser.playback_generation, 3);
    });
    std::fs::write(directory.0.join("second.wav"), mono_pcm_wav(16)).unwrap();
    editor.update(visual, |view, cx| {
        // This section verifies manual Right-arrow independently of auto selection.
        view.browser.auto_audition = false;
        view.browser.search.update(cx, |input, cx| input.clear(cx));
        view.refresh_sample_catalog(cx);
        cx.notify();
    });
    visual.run_until_parked();
    visual.simulate_keystrokes("down right up right");
    editor.read_with(visual, |view, _| {
        assert_eq!(view.browser.selected_index, 0);
        assert_eq!(view.browser.playback_generation, 5);
        assert!(view.pending_requests.is_empty());
    });
    // Enter keeps the existing explicit import path.
    editor.update(visual, |view, _| view.selected_track = Some(track));
    visual.simulate_keystrokes("enter");
    editor.read_with(visual, |view, _| {
        assert_eq!(view.browser.playback_generation, 5);
        let (imported_path, imported_track, _, _) = view
            .last_enqueued_import_to_track
            .as_ref()
            .expect("enter enqueues import to selected track");
        assert_eq!(imported_path, &path);
        assert_eq!(*imported_track, track);
    });
}

#[gpui::test]
fn browser_right_opens_selected_folder_without_sample_or_project_commands(cx: &mut TestAppContext) {
    cx.update(super::super::bind_editor_keys);
    let (handle, editor, _, directory, _) = browser_drag_fixture(cx);
    let folder = directory.0.join("folder");
    std::fs::create_dir(&folder).unwrap();
    let child = folder.join("project.hex");
    std::fs::write(&child, []).unwrap();
    let visual = &mut VisualTestContext::from_window(handle, cx);
    editor.update(visual, |view, cx| {
        view.browser.section = BrowserSection::Projects;
        view.browser.listing.as_mut().unwrap().entries = vec![LibraryEntry {
            name: "folder".into(),
            path: folder.clone(),
            kind: EntryKind::Directory,
        }];
        cx.notify();
    });
    visual.run_until_parked();
    let disclosure = visual.debug_bounds("browser-disclosure-0").unwrap();
    assert_eq!(
        editor.read_with(visual, |view, _| view.browser.tree.disclosure(&folder)),
        super::tree::Disclosure::Collapsed
    );
    struct RestoreTheme(hex_preferences::ThemePreferences);
    impl Drop for RestoreTheme {
        fn drop(&mut self) {
            crate::theme::apply_preferences(self.0);
        }
    }
    let original = crate::theme::preferences();
    let _restore = RestoreTheme(original);
    for (background, text) in [(0x20, 0xe0), (0xe0, 0x20)] {
        let mut preferences = original;
        preferences.background = hex_preferences::RgbColor::new(background, background, background);
        preferences.text = hex_preferences::RgbColor::new(text, text, text);
        crate::theme::apply_preferences(preferences);
        editor.update(visual, |_, cx| cx.notify());
        visual.run_until_parked();
        let expected = gpui::Hsla::from(gpui::rgb(crate::theme::color(
            crate::theme::PaletteRole::Muted,
        )));
        assert!(
            visual
                .rendered_monochrome_colors("browser-disclosure-0")
                .contains(&expected)
        );
        assert_eq!(
            visual.debug_bounds("browser-disclosure-0"),
            Some(disclosure)
        );
    }
    let row = visual.debug_bounds("browser-entry-0").unwrap();
    visual.simulate_mouse_down(row.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(row.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("right");
    visual.run_until_parked();
    assert!(
        !visual
            .rendered_monochrome_colors("browser-disclosure-0")
            .is_empty()
    );
    assert_eq!(
        visual.debug_bounds("browser-disclosure-0"),
        Some(disclosure)
    );
    editor.read_with(visual, |view, cx| {
        assert_eq!(view.browser.directory, directory.0);
        assert_eq!(
            view.browser.tree.disclosure(&folder),
            super::tree::Disclosure::Expanded
        );
        assert_eq!(view.browser_visible_entries(cx)[1].path, child);
        assert_eq!(view.browser.playback_generation, 0);
        assert!(view.pending_requests.is_empty());
        assert!(view.pending_audio_commands.is_empty());
    });
}

#[gpui::test]
fn split_browser_keeps_results_between_sidebar_tags_and_preview(cx: &mut TestAppContext) {
    let (handle, editor, _, _directory, _) = browser_drag_fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let sidebar = visual.debug_bounds("browser-sidebar").expect("sidebar");
    let row = visual.debug_bounds("browser-entry-0").expect("sample row");
    let tags = visual
        .debug_bounds("browser-tag-selector")
        .expect("tag selector");
    let preview = visual
        .debug_bounds("browser-selection-preview")
        .expect("preview");
    assert!(row.left() >= sidebar.right());
    assert!(row.top() >= tags.bottom());
    assert!(row.bottom() <= preview.top());
    let toggle = visual
        .debug_bounds("browser-control-≡")
        .expect("filter toggle");
    visual.simulate_mouse_down(toggle.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(toggle.center(), MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("browser-tag-selector").is_none());
    let before_editor_row = visual.debug_bounds("browser-entry-0").expect("sample row");
    let before_editor_preview = visual
        .debug_bounds("browser-selection-preview")
        .expect("preview");
    let open = visual
        .debug_bounds("browser-control-⋮")
        .expect("editor toggle");
    visual.simulate_mouse_down(open.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(open.center(), MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let tag_editor = visual
        .debug_bounds("browser-tag-editor")
        .expect("tag editor");
    let row = visual.debug_bounds("browser-entry-0").expect("sample row");
    assert!(
        row.right() <= tag_editor.left(),
        "row {row:?}, tag editor {tag_editor:?}"
    );
    assert_eq!(
        row, before_editor_row,
        "opening the editor preserves results geometry"
    );
    assert_eq!(
        visual
            .debug_bounds("browser-selection-preview")
            .expect("preview"),
        before_editor_preview
    );
    let close = visual
        .debug_bounds("browser-control-⊗")
        .expect("close editor");
    visual.simulate_mouse_down(close.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(close.center(), MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert_eq!(
        visual.debug_bounds("browser-entry-0").expect("sample row"),
        before_editor_row
    );
    // Reopen through the selected item's mini strip as well.
    let open = visual
        .debug_bounds("browser-control-⋯")
        .expect("mini editor toggle");
    visual.simulate_mouse_down(open.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(open.center(), MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert_eq!(
        visual.debug_bounds("browser-entry-0").expect("sample row"),
        before_editor_row
    );
    // The outer edge follows the pointer even with the extra editor column open.
    let outer = visual
        .debug_bounds("browser-resize-handle")
        .expect("outer divider");
    let target = point(outer.left() - px(30.0), outer.center().y);
    visual.simulate_mouse_down(outer.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let resized = visual
        .debug_bounds("browser-resize-handle")
        .expect("resized outer divider");
    assert!((resized.left() - target.x).abs() < px(1.0));
    editor.update(visual, |view, cx| {
        view.browser.width = super::MIN_WIDTH;
        cx.notify();
    });
    visual.run_until_parked();
    let row = visual
        .debug_bounds("browser-entry-0")
        .expect("compact sample row");
    let preview = visual
        .debug_bounds("browser-selection-preview")
        .expect("compact preview");
    assert!(row.size.width > px(0.0));
    assert!(row.bottom() <= preview.top());
}

#[gpui::test]
fn samples_catalog_combines_roots_and_active_place_indexed_search(cx: &mut TestAppContext) {
    let (handle, editor, _, first, original) = browser_drag_fixture(cx);
    let second = PreferencesTestDirectory::new();
    let nested = second.0.join("drums");
    std::fs::create_dir_all(&nested).unwrap();
    let other = nested.join("loop.WAV");
    std::fs::write(&other, mono_pcm_wav(16)).unwrap();
    let visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    editor.update(cx, |view, cx| {
        view.browser.places = vec![first.0.clone(), second.0.clone()];
        view.browser.section = BrowserSection::Samples;
        view.browser.search.update(cx, |input, cx| {
            input.set_text("loop", cx);
            cx.notify();
        });
        view.refresh_sample_catalog(cx);
        // Rapid refreshes must coalesce; the latest roots must win.
        view.refresh_sample_catalog(cx);
    });
    visual.run_until_parked();
    editor.update(cx, |view, cx| {
        let entries = view.browser_visible_entries(cx);
        assert_eq!(entries.len(), 2);
        assert!(
            entries
                .iter()
                .all(|entry| entry.kind == EntryKind::WaveAudio)
        );
        assert!(entries.iter().any(|entry| entry.path == original));
        assert!(entries.iter().any(|entry| entry.path == other));
        view.select_browser_source(BrowserSection::Place(second.0.clone()), cx);
        view.browser.search.update(cx, |input, cx| {
            input.set_text("DRUMS loop", cx);
            cx.notify();
        });
        assert!(view.browser_visible_entries(cx).is_empty());
        assert!(view.browser.loading);
        assert_eq!(view.browser.directory, second.0);
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, cx| {
        let filtered = view.browser_visible_entries(cx);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].path, other);
    });
    std::fs::remove_file(&other).unwrap();
    editor.update(cx, |view, cx| view.refresh_sample_catalog(cx));
    visual.run_until_parked();
    editor.read_with(cx, |view, cx| {
        assert!(view.browser_visible_entries(cx).is_empty());
        assert_eq!(view.browser.samples.catalog.entries().len(), 1);
        let database = view
            .browser
            .preferences_path
            .with_extension("samples-v1.db");
        assert_eq!(
            hex_library::catalog::SampleCatalog::load(&database).unwrap(),
            view.browser.samples.catalog
        );
    });
}

#[gpui::test]
fn browser_picker_resizes_to_icons_and_back_without_changing_browser_width(
    cx: &mut TestAppContext,
) {
    let (handle, editor, _, _directory, _) = browser_drag_fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let sidebar = visual.debug_bounds("browser-sidebar").expect("sidebar");
    let row = visual.debug_bounds("browser-entry-0").expect("sample row");
    let outer_before = visual
        .debug_bounds("browser-resize-handle")
        .expect("outer divider");
    let preview_before = visual
        .debug_bounds("browser-selection-preview")
        .expect("preview");
    let handle = visual
        .debug_bounds("browser-sidebar-resize")
        .expect("picker divider");
    let target = point(sidebar.left() + px(20.0), handle.center().y);
    visual.simulate_mouse_down(handle.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let compact = visual.debug_bounds("browser-sidebar").expect("icon rail");
    assert_eq!(
        compact.size.width,
        visual.debug_bounds("browser-control-⚙").unwrap().size.width + px(1.0)
    );
    let compact_row = visual
        .debug_bounds("browser-entry-0")
        .expect("expanded results");
    assert!(compact_row.left() < row.left());
    let compact_preview = visual
        .debug_bounds("browser-selection-preview")
        .expect("wider preview");
    let outer = visual
        .debug_bounds("browser-resize-handle")
        .expect("outer divider");
    assert_eq!(outer, outer_before);
    assert!(compact_preview.size.width > preview_before.size.width);
    assert_eq!(compact_preview.right(), outer.left());
    assert_eq!(compact_preview.left(), compact_row.left());
    let section = visual
        .debug_bounds("browser-section-projects")
        .expect("projects icon");
    visual.simulate_mouse_move(section.center(), None, Modifiers::default());
    visual.simulate_mouse_down(section.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(section.center(), MouseButton::Left, Modifiers::default());
    editor.read_with(visual, |view, _| {
        assert_eq!(view.browser.section, BrowserSection::Projects)
    });
    visual.run_until_parked();
    let handle = visual
        .debug_bounds("browser-sidebar-resize")
        .expect("compact divider");
    let target = point(sidebar.right() + px(2.0), handle.center().y);
    visual.simulate_mouse_down(handle.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert_eq!(
        visual
            .debug_bounds("browser-sidebar")
            .expect("restored sidebar")
            .size
            .width,
        sidebar.size.width
    );
}

#[gpui::test]
fn all_search_unions_samples_folder_and_devices_and_preserves_identity_on_refresh(
    cx: &mut TestAppContext,
) {
    cx.update(super::super::bind_editor_keys);
    let (handle, editor, _, indexed, _) = browser_drag_fixture(cx);
    let nested = indexed.0.join("nested");
    std::fs::create_dir_all(&nested).unwrap();
    let indexed_wav = nested.join("utility-index.wav");
    std::fs::write(&indexed_wav, mono_pcm_wav(16)).unwrap();
    let outside = PreferencesTestDirectory::new();
    let outside_wav = outside.0.join("utility-outside.wav");
    std::fs::write(&outside_wav, mono_pcm_wav(16)).unwrap();
    let project = outside.0.join("utility.hex");
    std::fs::write(&project, []).unwrap();
    let visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    editor.update(cx, |view, cx| {
        view.browser.directory = outside.0.clone();
        view.browser.listing = Some(hex_library::scan_directory(&outside.0).unwrap());
        view.browser.samples.invalidate_listing();
        // Global search must ignore an unrelated previous Place source.
        view.browser.section = BrowserSection::Place(indexed.0.join("unmatched-source"));
        view.apply_sample_catalog(
            hex_library::catalog::scan_samples(std::slice::from_ref(&indexed.0)).catalog,
            cx,
        );
        cx.notify();
    });
    cx.simulate_keystrokes(handle, "cmd-f");
    cx.simulate_input(handle, "utility");
    visual.run_until_parked();
    let selected_device = editor.update(cx, |view, cx| {
        assert_eq!(view.browser.section, BrowserSection::All);
        let entries = view.browser_visible_entries(cx);
        assert_eq!(entries.len(), 3);
        for path in [&indexed_wav, &outside_wav, &project] {
            assert_eq!(
                entries.iter().filter(|entry| &entry.path == path).count(),
                1
            );
        }
        assert!(Arc::ptr_eq(&entries, &view.browser_visible_entries(cx)));
        let devices = view.browser_visible_devices(cx);
        let device_index = devices
            .iter()
            .position(|(_, entry)| entry.name.to_lowercase() == "utility")
            .unwrap();
        let identity = (devices[device_index].0, devices[device_index].1.type_id);
        view.browser.selected_index = view
            .browser_history_index(&super::BrowserItemKey::Device(identity.0, identity.1), cx)
            .unwrap();
        view.browser.results_focused = true;
        cx.notify();
        identity
    });
    std::fs::write(nested.join("utility-second.wav"), mono_pcm_wav(16)).unwrap();
    editor.update(cx, |view, cx| view.refresh_sample_catalog(cx));
    visual.run_until_parked();
    editor.read_with(cx, |view, cx| {
        assert_eq!(view.browser_visible_entries(cx).len(), 4);
        assert!(matches!(view.browser_selected_item(cx), Some(super::BrowserItemKey::Device(class, id)) if (class, id) == selected_device));
        assert_eq!(view.browser_selected_label(cx).to_lowercase(), "utility");
    });
    let unchanged = editor.read_with(cx, |view, cx| view.browser_visible_entries(cx));
    editor.update(cx, |view, cx| view.refresh_sample_catalog(cx));
    visual.run_until_parked();
    editor.read_with(cx, |view, cx| {
        assert!(Arc::ptr_eq(&unchanged, &view.browser_visible_entries(cx)))
    });
    std::fs::write(outside.0.join("utility-third.wav"), mono_pcm_wav(16)).unwrap();
    editor.update(cx, |view, cx| {
        view.request_browser_scan(outside.0.clone(), cx)
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, cx| {
        assert_eq!(view.browser_visible_entries(cx).len(), 5);
        assert!(matches!(view.browser_selected_item(cx), Some(super::BrowserItemKey::Device(class, id)) if (class, id) == selected_device));
    });
    cx.simulate_keystrokes(handle, "enter");
    editor.read_with(cx, |view, _| {
        assert!(view.status.contains("drag a browser device"));
        assert!(view.last_enqueued_import_to_track.is_none());
    });
    editor.update(cx, |view, cx| view.request_browser_scan(nested.clone(), cx));
    visual.run_until_parked();
    editor.read_with(cx, |view, cx| {
        let entries = view.browser_visible_entries(cx);
        assert_eq!(
            entries.len(),
            2,
            "overlapping folder and catalog WAV paths appear once"
        );
        assert!(
            entries
                .iter()
                .all(|entry| entry.kind == EntryKind::WaveAudio)
        );
        assert_eq!(
            entries
                .iter()
                .filter(|entry| entry.path == indexed_wav)
                .count(),
            1
        );
    });
}

#[gpui::test]
fn browser_audition_strip_is_one_bounded_row(cx: &mut TestAppContext) {
    let (handle, editor, _, _directory, _) = browser_drag_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    for width in [220.0, 300.0, 480.0] {
        editor.update(&mut visual, |view, cx| {
            view.browser.width = width;
            cx.notify();
        });
        visual.run_until_parked();
        let preview = visual
            .debug_bounds("browser-selection-preview")
            .expect("preview");
        assert!(
            preview.size.height <= px(50.0),
            "compact preview was {preview:?}"
        );
        let row = visual
            .debug_bounds("browser-audition-row")
            .expect("horizontal audition row");
        let auto = visual
            .debug_bounds("browser-audition-auto")
            .expect("auto toggle");
        let transport = visual
            .debug_bounds("browser-audition-transport")
            .expect("play/stop");
        let waveform = visual
            .debug_bounds("browser-audition-waveform")
            .expect("tiny waveform");
        let volume = visual
            .debug_bounds("browser-audition-volume")
            .expect("volume slider");
        assert_eq!(row.size.height, px(22.0));
        assert!(row.top() >= preview.top() && row.bottom() <= preview.bottom());
        assert!(auto.right() <= transport.left() && transport.right() <= waveform.left());
        assert!(waveform.right() <= volume.left());
        assert_eq!(volume.right(), row.right());
        assert!(auto.left() >= row.left());
        for control in [auto, transport, waveform, volume] {
            assert!(control.top() >= row.top() && control.bottom() <= row.bottom());
        }
    }
}

#[gpui::test]
fn browser_audition_volume_drag_preserves_playback_position_and_selection_level(
    cx: &mut TestAppContext,
) {
    let (handle, editor, _, directory, _) = browser_drag_fixture(cx);
    std::fs::write(directory.0.join("next.wav"), mono_pcm_wav(16)).unwrap();
    let mut visual = VisualTestContext::from_window(handle, cx);
    editor.update(&mut visual, |view, cx| {
        view.browser.auto_audition = false;
        view.refresh_browser_tree(cx);
        view.browser.playback_fraction = 0.25;
        cx.notify();
    });
    visual.run_until_parked();
    let volume = visual.debug_bounds("browser-audition-volume").unwrap();
    let down = volume.center();
    let quieter = down + point(px(0.0), px(11.0));
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(quieter, Some(MouseButton::Left), Modifiers::default());
    visual.run_until_parked();
    visual.simulate_mouse_up(quieter, MouseButton::Left, Modifiers::default());
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.browser.audition_gain, 0.5);
        assert_eq!(view.browser.playback_generation, 0);
        assert_eq!(view.browser.playback_fraction, 0.25);
        assert!(!view.browser.audition_gain_dragging);
    });

    let next = visual.debug_bounds("browser-entry-1").unwrap().center();
    visual.simulate_mouse_down(next, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(next, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.browser.selected_index, 1);
        assert_eq!(view.browser.audition_gain, 0.5);
        assert_eq!(view.browser.playback_generation, 0);
    });
}

#[gpui::test]
fn browser_audition_toggle_keeps_manual_play_and_result_focus(cx: &mut TestAppContext) {
    cx.update(super::super::bind_editor_keys);
    let (handle, editor, _, _directory, _) = browser_drag_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let auto = visual.debug_bounds("browser-audition-auto").unwrap();
    visual.simulate_mouse_down(auto.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(auto.center(), MouseButton::Left, Modifiers::default());
    let row = visual.debug_bounds("browser-entry-0").unwrap();
    visual.simulate_mouse_down(row.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(row.center(), MouseButton::Left, Modifiers::default());
    editor.read_with(&visual, |view, _| {
        assert!(!view.browser.auto_audition);
        assert_eq!(view.browser.playback_generation, 0);
        assert!(view.browser.results_focused);
    });
    let transport = visual.debug_bounds("browser-audition-transport").unwrap();
    visual.simulate_mouse_down(transport.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(transport.center(), MouseButton::Left, Modifiers::default());
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.browser.playback_generation, 1);
        assert!(view.browser.results_focused);
        assert_eq!(
            view.browser.playback_phase,
            hex_audio::sample_preview::Phase::Unavailable
        );
        assert!(
            view.error
                .as_ref()
                .is_some_and(|error| error.contains("sample preview"))
        );
        assert_eq!(view.status.as_str(), "sample preview unavailable");
        assert!(view.pending_audio_commands.is_empty());
    });
    // The control must retain browser key focus so Right remains manual audition.
    visual.simulate_keystrokes("right");
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.browser.playback_generation, 2)
    });
    visual.simulate_mouse_down(auto.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(auto.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_down(row.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(row.center(), MouseButton::Left, Modifiers::default());
    editor.read_with(&visual, |view, _| {
        assert!(view.browser.auto_audition);
        assert_eq!(view.browser.playback_generation, 3);
        assert!(view.pending_requests.is_empty());
    });
}

#[gpui::test]
fn browser_audition_row_observes_pcm_completion_and_stop_without_project_transport(
    cx: &mut TestAppContext,
) {
    use hex_audio::sample_preview::Phase;
    let (handle, editor, _, _directory, _) = browser_drag_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    let (mut control, mut callback) = hex_audio::prepared::channel(48_000).unwrap();
    let generation = control.sample_preview.request().unwrap();
    control
        .sample_preview
        .publish(
            generation,
            hex_engine::Audio::new(48_000, vec![[0.25, -0.25]; 3]).unwrap(),
        )
        .unwrap();
    callback.begin_block();
    callback.render(&mut [[0.0; 2]; 2]).unwrap();
    editor.update(&mut visual, |view, cx| {
        assert!(view.accept_browser_sample_phase(control.sample_preview.snapshot().phase));
        assert_eq!(view.browser.playback_phase, Phase::Playing);
        assert_eq!(view.status.as_str(), "sample preview playing");
        cx.notify();
    });
    visual.run_until_parked();
    callback.render(&mut [[0.0; 2]; 2]).unwrap();
    assert_eq!(control.sample_preview.snapshot().phase, Phase::Finished);
    editor.update(&mut visual, |view, cx| {
        assert!(view.accept_browser_sample_phase(control.sample_preview.snapshot().phase));
        assert_eq!(view.status.as_str(), "sample preview finished");
        assert!(
            !view.accept_browser_sample_phase(Phase::Finished),
            "no idle timer repaint"
        );
        cx.notify();
    });
    // A later audition has its own real callback start observation.
    let next = control.sample_preview.request().unwrap();
    control
        .sample_preview
        .publish(
            next,
            hex_engine::Audio::new(48_000, vec![[0.2; 2]; 8]).unwrap(),
        )
        .unwrap();
    callback.begin_block();
    callback.render(&mut [[0.0; 2]; 2]).unwrap();
    editor.update(&mut visual, |view, cx| {
        assert!(view.accept_browser_sample_phase(control.sample_preview.snapshot().phase));
        cx.notify();
    });
    visual.run_until_parked();
    let stop = visual.debug_bounds("browser-audition-transport").unwrap();
    visual.simulate_mouse_down(stop.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(stop.center(), MouseButton::Left, Modifiers::default());
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.browser.playback_phase, Phase::Idle);
        assert_eq!(view.status.as_str(), "sample preview stopped");
        assert_eq!(view.browser.playback_generation, 1);
        assert!(view.browser.playback_output.is_none());
        assert!(view.browser.playback_pending.is_none());
        assert!(view.pending_audio_commands.is_empty());
    });
    editor.update(&mut visual, |view, cx| {
        assert!(
            !view.poll_browser_sample_playback(),
            "Stop removes the observed output token"
        );
        assert!(view.accept_browser_sample_phase(Phase::Unavailable));
        assert!(
            view.error
                .as_ref()
                .is_some_and(|error| error.contains("sample preview"))
        );
        cx.notify();
    });
}

#[gpui::test]
fn browser_auto_navigation_auditions_new_selection_but_not_boundary_repeats(
    cx: &mut TestAppContext,
) {
    cx.update(super::super::bind_editor_keys);
    let (handle, editor, _, directory, _) = browser_drag_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    std::fs::write(directory.0.join("second.wav"), mono_pcm_wav(16)).unwrap();
    editor.update(&mut visual, |view, cx| {
        view.refresh_browser_tree(cx);
        cx.notify();
    });
    visual.run_until_parked();
    editor.update(&mut visual, |view, cx| {
        view.browser.results_focused = false;
        view.browser.selected_index = 0;
        view.move_browser_selection(true, cx);
    });
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.browser.playback_generation, 1)
    });
    editor.update(&mut visual, |view, cx| {
        view.move_browser_selection(true, cx)
    });
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.browser.selected_index, 1);
        assert_eq!(view.browser.playback_generation, 2);
    });
    editor.update(&mut visual, |view, cx| {
        view.move_browser_selection(true, cx)
    });
    editor.read_with(&visual, |view, _| {
        assert_eq!(
            view.browser.playback_generation, 2,
            "holding Down at the last row cannot replay it"
        )
    });
    editor.update(&mut visual, |view, cx| {
        view.browser.auto_audition = false;
        view.move_browser_selection(false, cx);
    });
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.browser.selected_index, 0);
        assert_eq!(view.browser.playback_generation, 2);
    });
}

#[gpui::test]
fn browser_result_reset_cancels_preparing_and_playing_audition(cx: &mut TestAppContext) {
    let (handle, editor, _, _, _) = browser_drag_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    editor.update(&mut visual, |view, _| {
        for phase in [
            hex_audio::sample_preview::Phase::Preparing,
            hex_audio::sample_preview::Phase::Playing,
        ] {
            view.browser.playback_phase = phase;
            view.browser.playback_output = Some((1, 1));
            view.browser.playback_path = Some("obsolete.wav".into());
            let before = view.browser.playback_generation;
            view.reset_browser_results();
            assert_eq!(view.browser.playback_generation, before + 1);
            assert!(view.browser.playback_output.is_none());
            assert!(view.browser.playback_pending.is_none());
            assert!(view.browser.playback_path.is_none());
            assert_eq!(
                view.browser.playback_phase,
                hex_audio::sample_preview::Phase::Idle
            );
        }
    });
}

#[gpui::test]
fn browser_superseded_preview_preparations_are_cancelled(cx: &mut TestAppContext) {
    use std::sync::atomic::{AtomicBool, Ordering};
    let (handle, editor, _, directory, _) = browser_drag_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    std::fs::write(directory.0.join("second.wav"), mono_pcm_wav(16)).unwrap();
    editor.update(&mut visual, |view, cx| {
        view.refresh_browser_tree(cx);
        cx.notify();
    });
    visual.run_until_parked();
    editor.update(&mut visual, |view, cx| {
        // An audition still decoding when the selection moves on is cancelled.
        let audition = Arc::new(AtomicBool::new(false));
        view.browser.playback_in_flight = Some(Arc::clone(&audition));
        view.browser.results_focused = false;
        view.browser.selected_index = 0;
        view.move_browser_selection(true, cx);
        assert!(audition.load(Ordering::Relaxed), "new selection cancels it");
        let stopped = Arc::new(AtomicBool::new(false));
        view.browser.playback_in_flight = Some(Arc::clone(&stopped));
        view.cancel_browser_sample_playback();
        assert!(stopped.load(Ordering::Relaxed), "stop cancels it");
        view.browser.playback_in_flight = None;

        let key = |name: &str| super::PreviewKey {
            path: directory.0.join(name),
            sample_rate: 48_000,
        };
        let job = |key| super::PreviewRequest {
            key,
            slot: super::PreviewSlot::default(),
            cancel: Arc::new(AtomicBool::new(false)),
        };
        let selected = job(key("selected.wav"));
        view.browser.preview_selected = Some(selected.key.clone());
        view.browser.preview_in_flight = Some(selected.clone());
        view.queue_browser_audio_preview(key("hovered.wav"), super::PreviewSlot::default(), cx);
        assert!(
            !selected.cancel.load(Ordering::Relaxed),
            "the selected row keeps its waveform job"
        );
        let hovered = job(key("hovered.wav"));
        view.browser.preview_in_flight = Some(hovered.clone());
        view.queue_browser_audio_preview(key("later.wav"), super::PreviewSlot::default(), cx);
        assert!(
            hovered.cancel.load(Ordering::Relaxed),
            "a superseded hover waveform job is cancelled"
        );
        assert_eq!(
            view.browser
                .preview_pending
                .as_ref()
                .map(|pending| &pending.key),
            Some(&key("later.wav"))
        );
        view.browser.preview_in_flight = None;
        view.browser.preview_pending = None;
    });
}

#[gpui::test]
fn browser_hover_and_selection_keep_selected_waveform_and_surroundings_stable(
    cx: &mut TestAppContext,
) {
    let (handle, editor, track, directory, path) = browser_drag_fixture(cx);
    for i in 0..40 {
        std::fs::write(
            directory.0.join(format!("sample-{i:02}.wav")),
            mono_pcm_wav(32),
        )
        .unwrap();
    }
    let metadata = Rc::new(
        hex_session::service::PreviewSources::default()
            .prepare_waveform(&path, 48_000, &std::sync::atomic::AtomicBool::new(false))
            .unwrap(),
    );
    let mut visual = VisualTestContext::from_window(handle, cx);
    editor.update(&mut visual, |view, cx| {
        view.browser.auto_audition = false;
        view.browser.listing = Some(hex_library::scan_directory(&directory.0).unwrap());
        view.browser.samples.invalidate_listing();
        view.apply_sample_catalog(
            hex_library::catalog::scan_samples(std::slice::from_ref(&directory.0)).catalog,
            cx,
        );
        view.browser.results_focused = true;
        view.browser.selected_index = 0;
        let slot = view.browser_preview_cell(path.clone(), 48_000);
        *slot.metadata.borrow_mut() = Some(metadata.clone());
        // Uniform-list rendering may compose more rows than cache capacity.
        let entries = view.browser_visible_entries(cx);
        let cached_before = view.browser.preview_cache.len();
        for (index, entry) in entries.iter().cloned().enumerate() {
            let _ = view.browser_row(index, entry, cx);
        }
        assert_eq!(
            view.browser.preview_cache.len(),
            cached_before,
            "rendering visible rows cannot mutate the preparation cache"
        );
        let cached = view.browser_preview_cell(path.clone(), 48_000);
        assert!(
            cached
                .metadata
                .borrow()
                .as_ref()
                .is_some_and(|value| Rc::ptr_eq(value, &metadata)),
            "rendering other rows must not evict/reprepare the selected waveform"
        );
        cx.notify();
    });
    visual.run_until_parked();
    let lane = Box::leak(format!("arrangement-lane-{}", track.value()).into_boxed_str());
    let bounds = visual.debug_bounds(lane).unwrap();
    let color = visual.rendered_background(lane);
    let canonical = editor.read_with(&visual, |view, _| view.state.clone().unwrap());
    for index in 0..3 {
        let row = visual
            .debug_bounds(Box::leak(format!("browser-entry-{index}").into_boxed_str()))
            .unwrap();
        visual.simulate_mouse_down(row.center(), MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(row.center(), MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        let waveform = visual.debug_bounds("browser-audition-waveform").unwrap();
        let slot = editor.read_with(&visual, |view, cx| {
            let selected = view.browser_visible_entries(cx)[index].path.clone();
            view.browser.preview_cache[&super::PreviewKey {
                path: selected,
                sample_rate: 48_000,
            }]
                .clone()
        });
        let preparations = editor.read_with(&visual, |view, _| view.browser.preview_preparations);
        for _ in 0..4 {
            visual.simulate_mouse_move(waveform.center(), None, Modifiers::default());
            visual.run_until_parked();
            editor.read_with(&visual, |view, _| {
                assert!(slot.metadata.borrow().is_some());
                assert_eq!(
                    view.browser.preview_preparations, preparations,
                    "hover cannot reprepare"
                );
                let key = view.browser.preview_selected.as_ref().unwrap();
                assert!(Rc::ptr_eq(
                    &view.browser.preview_cache[key].metadata,
                    &slot.metadata
                ));
                assert!(view.browser.preview_in_flight.is_none());
                assert!(view.browser.preview_pending.is_none());
                let source = super::super::overview_waveform::Source::Browser(Arc::clone(
                    &slot.metadata.borrow().as_ref().unwrap().waveform,
                ));
                let expected = super::super::waveform::waveform_column_count(
                    f32::from(waveform.size.width),
                    2.0,
                );
                assert!(expected > 48, "browser paint must use detailed columns");
                assert_eq!(
                    view.overview_waveform_cache
                        .borrow()
                        .prepared_columns(&source, waveform, 2.0),
                    Some(expected),
                    "actual browser canvas must populate its geometry cache at rendered bounds"
                );
                assert_eq!(view.browser.playback_generation, 0, "hover cannot audition");
                assert!(Arc::ptr_eq(view.state.as_ref().unwrap(), &canonical));
                assert!(view.pending_audio_commands.is_empty());
            });
            assert_eq!(visual.debug_bounds(lane).unwrap(), bounds);
            assert_eq!(visual.rendered_background(lane), color);
            assert_eq!(
                visual.debug_bounds("browser-audition-waveform").unwrap(),
                waveform
            );
        }
    }
    // Actual GPUI TestWindow is fixed at 2x; 1x geometry has a separate oracle,
    // while native 1x visual acceptance remains a distinct check.
    let scale = visual.update(|window, _| window.scale_factor());
    assert_eq!(scale, 2.0);
    for width in [960.0, 1280.0] {
        visual.simulate_resize(gpui::size(gpui::px(width), gpui::px(700.0)));
        visual.run_until_parked();
        let waveform = visual.debug_bounds("browser-audition-waveform").unwrap();
        editor.read_with(&visual, |view, _| {
            let key = view.browser.preview_selected.as_ref().unwrap();
            let slot = &view.browser.preview_cache[key];
            let metadata = slot.metadata.borrow();
            let source = super::super::overview_waveform::Source::Browser(Arc::clone(
                &metadata.as_ref().unwrap().waveform,
            ));
            let expected = super::super::waveform::waveform_column_count(
                f32::from(waveform.size.width),
                scale,
            );
            assert_eq!(
                view.overview_waveform_cache
                    .borrow()
                    .prepared_columns(&source, waveform, scale),
                Some(expected),
                "browser resize must repaint the detailed waveform at its actual new bounds"
            );
            assert_eq!(
                view.browser.playback_generation, 0,
                "resize cannot audition"
            );
        });
    }
}

#[gpui::test]
fn browser_waveform_click_is_manual_positioned_audition_and_retains_focus(cx: &mut TestAppContext) {
    let (handle, editor, _, _directory, path) = browser_drag_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    editor.update(&mut visual, |view, cx| {
        view.browser.auto_audition = false;
        view.browser.results_focused = true;
        cx.notify();
    });
    visual.run_until_parked();
    let bounds = visual.debug_bounds("browser-audition-waveform").unwrap();
    let mut generation = 0;
    for fraction in [0.0, 0.5, 0.9, 0.25] {
        let position = point(
            bounds.left() + bounds.size.width * fraction,
            bounds.center().y,
        );
        visual.update(|window, cx| {
            editor.read_with(cx, |view, cx| {
                view.browser.search.read(cx).focus(window);
            })
        });
        visual.simulate_mouse_down(position, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(position, MouseButton::Left, Modifiers::default());
        generation += 1;
        visual.update(|window, cx| {
            editor.read_with(cx, |view, cx| {
                assert!(
                    !view
                        .browser
                        .search
                        .read(cx)
                        .focus_handle()
                        .is_focused(window)
                );
                assert_eq!(view.browser_key_context(window, cx), "hex_browser");
            })
        });
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.browser.playback_generation, generation);
            assert!((view.browser.playback_fraction - fraction).abs() < 0.02);
            assert_eq!(view.browser.playback_path.as_ref(), Some(&path));
            assert_eq!(
                view.browser.playback_phase,
                hex_audio::sample_preview::Phase::Unavailable
            );
            assert!(view.browser.results_focused);
            assert!(!view.browser.auto_audition);
            assert!(view.pending_audio_commands.is_empty());
            assert!(view.pending_requests.is_empty());
        });
    }
    visual.simulate_mouse_move(bounds.center(), None, Modifiers::default());
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.browser.playback_generation, generation)
    });
}

#[gpui::test]
fn browser_waveform_position_observes_rendered_pcm_not_click_or_decode(cx: &mut TestAppContext) {
    use hex_audio::sample_preview::Phase;
    let (_, editor, _, _, _) = browser_drag_fixture(cx);
    let (mut control, mut callback) = hex_audio::prepared::channel(48_000).unwrap();
    let token = control.sample_preview.request().unwrap();
    control
        .sample_preview
        .publish_from(
            token,
            hex_engine::Audio::new(48_000, vec![[0.5; 2]; 10]).unwrap(),
            5,
        )
        .unwrap();
    editor.update(cx, |view, _| {
        view.browser.playback_output = Some((1, token));
        assert!(view.accept_browser_sample_snapshot(Some(control.sample_preview.snapshot())));
        assert_eq!(view.browser.playback_phase, Phase::Preparing);
        assert_eq!(view.browser.playback_fraction, 0.5);
        assert!(!view.accept_browser_sample_snapshot(Some(control.sample_preview.snapshot())));
    });
    callback.begin_block();
    callback.render(&mut [[0.0; 2]; 2]).unwrap();
    editor.update(cx, |view, _| {
        assert!(view.accept_browser_sample_snapshot(Some(control.sample_preview.snapshot())));
        assert_eq!(view.browser.playback_phase, Phase::Playing);
        assert_eq!(view.browser.playback_fraction, 0.7);
    });
    callback.render(&mut [[0.0; 2]; 8]).unwrap();
    editor.update(cx, |view, _| {
        assert!(view.accept_browser_sample_snapshot(Some(control.sample_preview.snapshot())));
        assert_eq!(view.browser.playback_phase, Phase::Finished);
        assert_eq!(view.browser.playback_fraction, 1.0);
        assert!(view.browser.playback_output.is_none());
        assert!(!view.accept_browser_sample_snapshot(Some(control.sample_preview.snapshot())));
        view.cancel_browser_sample_playback();
        assert_eq!(view.browser.playback_fraction, 0.0);
        assert!(!view.accept_browser_sample_snapshot(Some(control.sample_preview.snapshot())));
        assert!(view.pending_audio_commands.is_empty());
    });
}

#[gpui::test]
fn browser_new_selection_clears_completed_position_without_auto_audition(cx: &mut TestAppContext) {
    let (_, editor, _, _, _) = browser_drag_fixture(cx);
    editor.update(cx, |view, cx| {
        view.browser.auto_audition = false;
        view.browser.playback_phase = hex_audio::sample_preview::Phase::Finished;
        view.browser.playback_fraction = 1.0;
        view.browser.playback_path = Some("old.wav".into());
        let generation = view.browser.playback_generation;
        view.browser_selection_audition(Some("new.wav".into()), cx);
        assert_eq!(view.browser.playback_generation, generation);
        assert_eq!(
            view.browser.playback_phase,
            hex_audio::sample_preview::Phase::Idle
        );
        assert_eq!(view.browser.playback_fraction, 0.0);
        assert!(view.browser.playback_path.is_none());
        assert!(view.pending_audio_commands.is_empty());
    });
}

#[gpui::test]
fn browser_source_groups_have_exactly_one_persistent_selected_row(cx: &mut TestAppContext) {
    cx.update(super::super::bind_editor_keys);
    let (handle, editor, _, directory, _) = browser_drag_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    editor.update(&mut visual, |view, _| view.browser.auto_audition = false);
    visual.run_until_parked();
    let selectors = [
        "browser-section-samples",
        "browser-section-devices",
        "browser-section-projects",
        "browser-section-all",
        "browser-place-0",
        "browser-collection-0",
    ];
    let selected = gpui::rgb(crate::theme::color(crate::theme::PaletteRole::Secondary)).into();
    let idle = gpui::rgb(crate::theme::color(crate::theme::PaletteRole::Surface)).into();
    let minimum = f32::from(visual.debug_bounds("browser-control-⚙").unwrap().size.width) + 1.0;
    let places = editor.read_with(&visual, |view, _| view.browser.places.clone());
    for width in [124.0, 64.0, 63.0, minimum] {
        let compact = width < 64.0;
        let divider = visual.debug_bounds("browser-sidebar-resize").unwrap();
        let sidebar = visual.debug_bounds("browser-sidebar").unwrap();
        let target = divider.center() + point(px(width) - sidebar.size.width, px(0.0));
        visual.simulate_mouse_down(divider.center(), MouseButton::Left, Modifiers::default());
        editor.read_with(&visual, |view, _| {
            assert!(view.browser.layout.sidebar_drag.is_some())
        });
        visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        assert_eq!(
            visual.debug_bounds("browser-sidebar").unwrap().size.width,
            px(width)
        );
        editor.read_with(&visual, |view, _| {
            assert!(view.browser.layout.sidebar_drag.is_none());
            assert_eq!(view.browser.places, places);
            assert!(!view.browser.auto_audition);
        });
        for active in [0, 4, 1, 4, 5, 0, 5, 4, 2, 3] {
            let bounds = visual.debug_bounds(selectors[active]).unwrap();
            let before_hover = editor.read_with(&visual, |view, _| {
                (
                    view.browser.section.clone(),
                    view.browser.playback_generation,
                    view.browser.preview_preparations,
                )
            });
            // Exercise the full row hit target, including blank space beyond its icon.
            let hit = point(bounds.right() - px(1.0), bounds.center().y);
            visual.simulate_mouse_move(hit, None, Modifiers::default());
            visual.run_until_parked();
            editor.read_with(&visual, |view, _| {
                assert_eq!(
                    (
                        view.browser.section.clone(),
                        view.browser.playback_generation,
                        view.browser.preview_preparations
                    ),
                    before_hover,
                    "picker hover cannot select, audition or prepare samples"
                );
            });
            visual.simulate_mouse_down(hit, MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_up(hit, MouseButton::Left, Modifiers::default());
            visual.run_until_parked();
            visual.update(|window, cx| {
                editor.read_with(cx, |view, cx| {
                    assert_eq!(view.browser_key_context(window, cx), "hex_browser");
                    assert!(
                        !view
                            .browser
                            .search
                            .read(cx)
                            .focus_handle()
                            .is_focused(window)
                    );
                    assert!(view.browser.results_focus.is_focused(window));
                })
            });
            let source = editor.read_with(&visual, |view, _| view.browser.section.clone());
            visual.simulate_keystrokes("down");
            editor.read_with(&visual, |view, _| {
                assert_eq!(view.browser.section, source);
                assert!(view.last_enqueued_import_to_track.is_none());
                assert!(view.last_enqueued_import_to_new_track.is_none());
            });
            for (index, selector) in selectors.iter().enumerate() {
                assert_eq!(
                    visual.rendered_background(selector),
                    Some(if index == active { selected } else { idle }),
                    "{selector} after selecting {} (compact={compact})",
                    selectors[active]
                );
            }
            editor.read_with(&visual, |view, cx| {
                assert!(view.pending_audio_commands.is_empty());
                if active == 4 {
                    assert!(
                        view.browser_visible_entries(cx)
                            .iter()
                            .all(|entry| entry.path.starts_with(&directory.0))
                    );
                    assert!(view.browser_visible_devices(cx).is_empty());
                }
                if active == 5 {
                    assert!(view.browser_visible_entries(cx).is_empty());
                    assert!(view.browser_visible_devices(cx).is_empty());
                }
            });
        }
    }
}

#[gpui::test]
fn browser_offline_and_removed_place_never_shows_stale_or_second_source(cx: &mut TestAppContext) {
    let (handle, editor, _, first, _) = browser_drag_fixture(cx);
    let offline = PreferencesTestDirectory::new();
    let ghost = offline.0.join("ghost.wav");
    std::fs::write(&ghost, mono_pcm_wav(16)).unwrap();
    let catalog = hex_library::catalog::scan_samples(std::slice::from_ref(&offline.0)).catalog;
    std::fs::remove_file(&ghost).unwrap();
    std::fs::remove_dir(&offline.0).unwrap();
    let mut visual = VisualTestContext::from_window(handle, cx);
    editor.update(&mut visual, |view, cx| {
        view.browser.places.push(offline.0.clone());
        view.browser.music = offline.0.clone();
        view.browser.home = first.0.clone();
        view.browser.samples.replace(catalog);
        view.browser.search.update(cx, |input, cx| {
            input.set_text("ghost", cx);
            cx.notify();
        });
        view.select_browser_source(BrowserSection::Place(offline.0.clone()), cx);
        cx.notify();
    });
    visual.run_until_parked();
    editor.read_with(&visual, |view, cx| {
        assert_eq!(
            view.browser.section,
            BrowserSection::Place(offline.0.clone())
        );
        assert_eq!(view.browser.directory, offline.0);
        assert!(view.browser.error.is_some());
        assert!(
            view.browser_visible_entries(cx).is_empty(),
            "offline place must not expose stale indexed matches"
        );
        assert!(view.browser.listing.is_none());
    });
    editor.update(&mut visual, |view, cx| {
        view.browser.places.retain(|path| *path != offline.0);
        cx.notify();
    });
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.browser.section, BrowserSection::Samples);
        assert!(
            view.browser
                .places_status
                .as_ref()
                .unwrap()
                .contains("selected place removed")
        );
    });
}

#[gpui::test]
fn browser_source_transition_discards_late_scan_and_keeps_query_and_keyboard_scope(
    cx: &mut TestAppContext,
) {
    cx.update(super::super::bind_editor_keys);
    let (handle, editor, _, directory, _) = browser_drag_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    editor.update(&mut visual, |view, cx| {
        view.browser.auto_audition = false;
        view.browser.search.update(cx, |input, cx| {
            input.set_text("loop", cx);
            cx.notify();
        });
        view.browser.scans = Default::default();
        let old = view.browser.scans.request(directory.0.clone()).unwrap();
        view.select_browser_source(BrowserSection::Collection(0), cx);
        view.browser_scan_finished(
            old,
            Ok(hex_library::scan_directory(&directory.0).unwrap()),
            cx,
        );
        assert!(view.browser_visible_entries(cx).is_empty());
        assert_eq!(view.browser.section, BrowserSection::Collection(0));
        view.select_browser_source(BrowserSection::Samples, cx);
        assert_eq!(view.browser.search.read(cx).text(), "loop");
        cx.notify();
    });
    visual.run_until_parked();
    let place = visual.debug_bounds("browser-place-0").unwrap();
    visual.simulate_mouse_down(place.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(place.center(), MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    visual.simulate_keystrokes("down");
    editor.read_with(&visual, |view, cx| {
        assert!(view.browser.results_focused);
        assert_eq!(view.browser_visible_entries(cx).len(), 1);
        assert_eq!(view.browser.search.read(cx).text(), "loop");
        let a = view.browser_visible_entries(cx);
        let b = view.browser_visible_entries(cx);
        assert!(
            Arc::ptr_eq(&a, &b),
            "unchanged source/query must retain rows"
        );
    });
    visual.simulate_keystrokes("cmd-f");
    editor.read_with(&visual, |view, cx| {
        assert_eq!(view.browser.section, BrowserSection::All);
        assert_eq!(view.browser.search.read(cx).text(), "loop");
    });
}

#[gpui::test]
fn browser_picker_glyph_alignment_and_continuous_minimum_geometry(cx: &mut TestAppContext) {
    let (handle, _editor, _, _directory, _) = browser_drag_fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let control = visual.debug_bounds("browser-control-⚙").unwrap();
    // Existing 22px browser controls supply the usable hit size; the sidebar
    // additionally owns its existing one-pixel right border.
    let minimum = f32::from(control.size.width) + 1.0;
    let rows = [
        ("browser-collection-0", "browser-picker-glyph-collection-0"),
        ("browser-collection-1", "browser-picker-glyph-collection-1"),
        ("browser-collection-2", "browser-picker-glyph-collection-2"),
        ("browser-section-samples", "browser-picker-glyph-section-0"),
        ("browser-section-devices", "browser-picker-glyph-section-1"),
        ("browser-section-projects", "browser-picker-glyph-section-2"),
        ("browser-section-all", "browser-picker-glyph-section-3"),
        ("browser-place-0", "browser-picker-glyph-place-0"),
    ];
    let outer_before = visual.debug_bounds("browser-resize-handle").unwrap();
    let mut measurements = Vec::new();
    for requested in [124.0, 80.0, 64.0, 63.0, 60.0, 48.0, 36.0, minimum] {
        let before = visual.debug_bounds("browser-sidebar").unwrap();
        let divider = visual.debug_bounds("browser-sidebar-resize").unwrap();
        let target = divider.center() + point(px(requested) - before.size.width, px(0.0));
        visual.simulate_mouse_down(divider.center(), MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        let sidebar = visual.debug_bounds("browser-sidebar").unwrap();
        let control = visual.debug_bounds("browser-control-⚙").unwrap();
        let glyphs = rows.map(|(row, glyph)| {
            let row_bounds = visual.debug_bounds(row).unwrap();
            let glyph_bounds = visual.debug_bounds(glyph).unwrap();
            println!("picker requested={requested} actual={} {glyph}: row={row_bounds:?} glyph={glyph_bounds:?} control={control:?}", f32::from(sidebar.size.width));
            (row_bounds, glyph_bounds)
        });
        measurements.push((requested, sidebar, control, glyphs));
        assert_eq!(
            visual.debug_bounds("browser-resize-handle").unwrap(),
            outer_before
        );
    }
    for (requested, sidebar, control, glyphs) in measurements {
        assert_eq!(
            sidebar.size.width,
            px(requested),
            "picker resize must be continuous down to the existing-control-derived minimum"
        );
        for (row, glyph) in glyphs {
            assert_eq!(
                glyph.center().x,
                control.center().x,
                "all picker glyphs must share the control column at width {requested}"
            );
            // GPUI rounds text origins to pixels while font line boxes may be fractional.
            assert!(f32::from(glyph.center().y - row.center().y).abs() <= 0.5);
            assert!(glyph.size.width <= control.size.width);
            assert!(row.contains(&glyph.origin));
            assert!(glyph.right() <= row.right());
            assert!(row.size.width >= control.size.width);
            assert_eq!(row.size.height, px(18.0));
        }
    }
}

#[gpui::test]
fn browser_picker_resize_retains_rows_and_clips_long_names(cx: &mut TestAppContext) {
    let (handle, editor, _, directory, _) = browser_drag_fixture(cx);
    editor.update(cx, |view, cx| {
        view.browser.places[0] = directory.0.join("a very long custom place name");
        cx.notify();
    });
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let selectors = [
        "browser-collection-0",
        "browser-collection-2",
        "browser-section-samples",
        "browser-section-all",
        "browser-control-⚙",
        "browser-place-0",
    ];
    // Repeat with the real section-settings controls open: narrowing must not
    // remove that block and move Places either.
    for settings_open in [false, true] {
        if settings_open {
            let gear = visual.debug_bounds("browser-control-⚙").unwrap();
            visual.simulate_click(gear.center(), Modifiers::default());
            visual.run_until_parked();
        }
        let baseline = selectors.map(|selector| visual.debug_bounds(selector).unwrap());
        for width in [124.0, 110.0, 80.0, 64.0, 63.0, 48.0, 27.0, 23.0, 124.0] {
            let sidebar = visual.debug_bounds("browser-sidebar").unwrap();
            let divider = visual.debug_bounds("browser-sidebar-resize").unwrap();
            let target = divider.center() + point(px(width) - sidebar.size.width, px(0.0));
            visual.simulate_mouse_down(divider.center(), MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
            visual.run_until_parked();
            for (selector, before) in selectors.into_iter().zip(baseline) {
                let after = visual.debug_bounds(selector).unwrap();
                assert_eq!(
                    after.origin, before.origin,
                    "{selector} must not reflow at width {width}"
                );
                assert_eq!(
                    after.size.height, before.size.height,
                    "{selector} row height at {width}"
                );
            }
            for (label, row) in [
                ("browser-picker-label-collection-0", "browser-collection-0"),
                ("browser-picker-label-section-0", "browser-section-samples"),
                ("browser-picker-label-place-0", "browser-place-0"),
            ] {
                let label = visual
                    .debug_bounds(label)
                    .expect("label remains mounted at zero width");
                let row = visual.debug_bounds(row).unwrap();
                let available = (row.right() - label.left()).max(px(0.0));
                assert_eq!(
                    label.size.width, available,
                    "label uses only remaining row width"
                );
                assert!(
                    label.size.height <= row.size.height,
                    "label must stay one line"
                );
                if width <= 27.0 {
                    assert_eq!(
                        label.size.width,
                        px(0.0),
                        "no label space beside icon/gap/border"
                    );
                }
            }
        }
    }
}

#[gpui::test]
fn browser_folder_double_click_expands_inline_without_replacing_surrounding_rows(
    cx: &mut TestAppContext,
) {
    let (handle, editor, track, directory, original) = browser_drag_fixture(cx);
    let folder = directory.0.join("a");
    std::fs::create_dir(&folder).unwrap();
    let child = folder.join("kick.wav");
    std::fs::write(&child, mono_pcm_wav(24)).unwrap();
    let sibling = directory.0.join("b");
    std::fs::create_dir(&sibling).unwrap();
    editor.update(cx, |view, cx| {
        view.browser.section = BrowserSection::Place(directory.0.clone());
        view.browser.auto_audition = false;
        view.request_browser_scan(directory.0.clone(), cx);
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let lane = Box::leak(format!("arrangement-lane-{}", track.value()).into_boxed_str());
    let before = visual.debug_bounds(lane).expect("arrangement lane");
    let row = visual.debug_bounds("browser-entry-0").unwrap();
    visual.simulate_event(gpui::MouseDownEvent {
        position: row.center(),
        button: MouseButton::Left,
        click_count: 2,
        first_mouse: false,
        modifiers: Modifiers::default(),
    });
    visual.simulate_mouse_up(row.center(), MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |view, cx| {
        assert_eq!(
            view.browser.directory, directory.0,
            "disclosure must not navigate to another page"
        );
        let entries = view.browser_visible_entries(cx);
        assert_eq!(
            entries.iter().map(|entry| &entry.path).collect::<Vec<_>>(),
            vec![&folder, &child, &sibling, &original]
        );
        assert!(view.pending_requests.is_empty());
        assert!(view.last_enqueued_import_to_track.is_none());
    });
    assert_eq!(
        before,
        visual
            .debug_bounds(lane)
            .expect("retained arrangement lane")
    );
}

#[gpui::test]
fn browser_inline_tree_nested_keyboard_refresh_and_hover_retain_identity(cx: &mut TestAppContext) {
    cx.update(super::super::bind_editor_keys);
    let (handle, editor, _, directory, _) = browser_drag_fixture(cx);
    let folder = directory.0.join("a");
    let nested = folder.join("nested");
    std::fs::create_dir_all(&nested).unwrap();
    let sample = nested.join("kick.wav");
    std::fs::write(&sample, mono_pcm_wav(32)).unwrap();
    editor.update(cx, |view, cx| {
        view.browser.auto_audition = false;
        view.request_browser_scan(directory.0.clone(), cx);
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let disclosure = visual.debug_bounds("browser-disclosure-0").unwrap();
    visual.simulate_mouse_down(disclosure.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(disclosure.center(), MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    // Click the nested folder label, then actual Right/Left actions in result focus.
    let row = visual.debug_bounds("browser-entry-1").unwrap();
    visual.simulate_mouse_down(row.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(row.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("right");
    visual.run_until_parked();
    let (requests, entries, state) = editor.read_with(&visual, |view, cx| {
        let entries = view.browser_visible_entries(cx);
        assert_eq!(entries[2].path, sample);
        assert_eq!(view.browser.tree.depth(&sample), 2);
        assert_eq!(view.browser.selected_index, 1);
        (
            view.browser.tree.requests_started,
            entries,
            view.state.clone().unwrap(),
        )
    });
    let sample_bounds = visual.debug_bounds("browser-entry-2").unwrap();
    for x in [0.2, 0.6, 0.9, 0.4] {
        visual.simulate_mouse_move(
            point(
                sample_bounds.left() + sample_bounds.size.width * x,
                sample_bounds.center().y,
            ),
            None,
            Modifiers::default(),
        );
        visual.run_until_parked();
    }
    editor.read_with(&visual, |view, cx| {
        assert_eq!(
            view.browser.tree.requests_started, requests,
            "hover never re-enumerates folders"
        );
        assert!(
            Arc::ptr_eq(&entries, &view.browser_visible_entries(cx)),
            "unchanged rows retain their snapshot"
        );
        assert!(Arc::ptr_eq(&state, view.state.as_ref().unwrap()));
    });
    visual.simulate_keystrokes("down");
    let added = nested.join("after.wav");
    std::fs::write(&added, mono_pcm_wav(64)).unwrap();
    editor.update(&mut visual, |view, cx| view.refresh_browser_tree(cx));
    visual.run_until_parked();
    editor.read_with(&visual, |view, cx| {
        let entries = view.browser_visible_entries(cx);
        assert_eq!(
            entries[view.browser.selected_index].path, sample,
            "refresh retains selected native path despite inserted row"
        );
        assert!(entries.iter().any(|entry| entry.path == added));
        assert_eq!(view.browser.directory, directory.0);
    });
    visual.simulate_keystrokes("left");
    editor.read_with(&visual, |view, cx| {
        assert_eq!(
            view.browser_visible_entries(cx)[view.browser.selected_index].path,
            nested
        )
    });
    visual.simulate_keystrokes("down");
    visual.simulate_keystrokes("down");
    editor.update(&mut visual, |view, cx| {
        assert_eq!(
            view.browser_visible_entries(cx)[view.browser.selected_index].path,
            sample
        );
        view.browser.playback_generation = 10;
        view.browser.playback_path = Some(sample.clone());
        view.browser.playback_output = Some((1, 1));
        view.browser.playback_phase = hex_audio::sample_preview::Phase::Preparing;
        view.browser.playback_pending = Some(super::PlaybackPreviewRequest {
            generation: 10,
            output_generation: 1,
            output_identity: 1,
            start_fraction: 0.5,
            path: sample.clone(),
            sample_rate: 48_000,
        });
    });
    let disclosure = visual.debug_bounds("browser-disclosure-1").unwrap();
    visual.simulate_mouse_down(disclosure.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(disclosure.center(), MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |view, cx| {
        assert!(
            !view
                .browser_visible_entries(cx)
                .iter()
                .any(|entry| entry.path == sample)
        );
        assert_eq!(
            view.browser_visible_entries(cx)[view.browser.selected_index].path,
            nested
        );
        assert_eq!(view.browser.playback_generation, 11);
        assert!(view.browser.playback_pending.is_none());
        assert!(view.browser.playback_output.is_none());
        assert_eq!(
            view.browser.playback_phase,
            hex_audio::sample_preview::Phase::Idle
        );
    });
    visual.simulate_keystrokes("left");
    visual.simulate_keystrokes("left");
    visual.run_until_parked();
    editor.read_with(&visual, |view, cx| {
        assert!(
            !view
                .browser_visible_entries(cx)
                .iter()
                .any(|entry| entry.path == nested)
        )
    });
}

#[gpui::test]
fn browser_tree_same_name_files_keep_distinct_preview_paths_and_bounded_labels(
    cx: &mut TestAppContext,
) {
    cx.update(super::super::bind_editor_keys);
    let (handle, editor, _, directory, _) = browser_drag_fixture(cx);
    let a = directory.0.join("a");
    let b = directory.0.join("b");
    std::fs::create_dir(&a).unwrap();
    std::fs::create_dir(&b).unwrap();
    let first = a.join("kick.wav");
    let second = b.join("kick.wav");
    std::fs::write(&first, mono_pcm_wav(32)).unwrap();
    std::fs::write(&second, mono_pcm_wav(64)).unwrap();
    editor.update(cx, |view, cx| {
        view.browser.auto_audition = false;
        view.browser.width = 220.0;
        view.request_browser_scan(directory.0.clone(), cx);
        view.refresh_sample_catalog(cx);
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    for (index, keep) in [(0, false), (2, true)] {
        let selector = Box::leak(format!("browser-disclosure-{index}").into_boxed_str());
        let disclosure = visual.debug_bounds(selector).unwrap();
        let modifiers = Modifiers {
            platform: keep,
            ..Modifiers::default()
        };
        visual.simulate_mouse_down(disclosure.center(), MouseButton::Left, modifiers);
        visual.simulate_mouse_up(disclosure.center(), MouseButton::Left, modifiers);
        visual.run_until_parked();
    }
    for (index, path, frames) in [(1, first, 32), (3, second, 64)] {
        let selector = Box::leak(format!("browser-entry-{index}").into_boxed_str());
        let label_selector = Box::leak(format!("browser-entry-label-{index}").into_boxed_str());
        let row = visual.debug_bounds(selector).unwrap();
        let label = visual.debug_bounds(label_selector).unwrap();
        assert!(label.left() >= row.left() && label.right() <= row.right());
        assert!(label.size.height <= row.size.height);
        visual.simulate_mouse_down(label.center(), MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(label.center(), MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        visual.simulate_keystrokes("right");
        visual.run_until_parked();
        editor.read_with(&visual, |view, cx| {
            assert_eq!(view.browser.playback_path.as_ref(), Some(&path));
            let entries = view.browser_visible_entries(cx);
            assert_eq!(entries[index].name, "kick.wav");
            assert_eq!(entries[index].path, path);
            let key = super::PreviewKey {
                path: path.clone(),
                sample_rate: 48_000,
            };
            assert_eq!(
                view.browser.preview_cache[&key]
                    .metadata
                    .borrow()
                    .as_ref()
                    .unwrap()
                    .playback_frames,
                frames
            );
            assert!(view.last_enqueued_import_to_track.is_none());
        });
    }
    editor.update(&mut visual, |view, cx| {
        view.select_browser_source(BrowserSection::Samples, cx);
        view.browser.search.update(cx, |input, cx| {
            input.set_text("kick", cx);
            cx.notify();
        });
    });
    visual.run_until_parked();
    editor.read_with(&visual, |view, cx| {
        let entries = view.browser_visible_entries(cx);
        assert_eq!(
            entries.len(),
            2,
            "indexed Samples search retains both native paths"
        );
        assert_eq!(entries[0].name, entries[1].name);
        assert_ne!(entries[0].path, entries[1].path);
    });
}

#[gpui::test]
fn browser_samples_count_tracks_hierarchy_items_and_flat_sample_matches(cx: &mut TestAppContext) {
    let (handle, editor, _, _directory, _) = browser_drag_fixture(cx);
    let first = PreferencesTestDirectory::new();
    let second = PreferencesTestDirectory::new();
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    editor.update(&mut visual, |view, cx| {
        view.browser.places = vec![first.0.clone(), second.0.clone()];
        view.browser.directory = first.0.clone();
        view.browser.section = BrowserSection::Samples;
        view.browser
            .search
            .update(cx, |input, cx| input.set_text("", cx));
        view.browser.samples.replace(Default::default());
        view.browser.tree.source_changed();
        assert_eq!(view.browser_visible_entries(cx).len(), 2);
        assert_eq!(view.sample_results_count_text(cx), "2 items");
        cx.notify();
    });
    let nested = first.0.join("nested");
    std::fs::create_dir(&nested).unwrap();
    std::fs::write(nested.join("kick.wav"), mono_pcm_wav(16)).unwrap();
    editor.update(&mut visual, |view, cx| {
        view.expand_browser_folder(first.0.clone(), cx)
    });
    visual.run_until_parked();
    editor.read_with(&visual, |view, cx| {
        let entries = view.browser_visible_entries(cx);
        assert_eq!(entries.len(), 3);
        assert!(
            entries
                .iter()
                .all(|entry| entry.kind == EntryKind::Directory)
        );
        assert_eq!(view.sample_results_count_text(cx), "3 items");
    });
    editor.update(&mut visual, |view, cx| {
        view.browser.search.update(cx, |input, cx| {
            input.set_text("kick", cx);
            cx.notify();
        });
        view.refresh_sample_catalog(cx);
    });
    visual.run_until_parked();
    editor.read_with(&visual, |view, cx| {
        assert_eq!(view.browser_visible_entries(cx).len(), 1);
        assert_eq!(view.sample_results_count_text(cx), "1 samples");
    });
    editor.update(&mut visual, |view, cx| {
        view.browser
            .search
            .update(cx, |input, cx| input.set_text("missing", cx));
        assert!(view.browser_visible_entries(cx).is_empty());
        assert_eq!(view.sample_results_count_text(cx), "0 samples");
    });
}

#[gpui::test]
fn place_correctness_reselect_gates_cached_rows_and_old_audition_callbacks(
    cx: &mut TestAppContext,
) {
    let (handle, editor, track, directory, path) = browser_drag_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    editor.update(&mut visual, |view, cx| {
        view.selected_track = Some(track);
        view.browser.section = BrowserSection::Place(directory.0.clone());
        view.browser.loading = false;
        view.browser.error = None;
        view.browser.scans = Default::default();
        // Hold the worker lane so the fresh request remains deterministically pending.
        let old = view.browser.scans.request(directory.0.clone()).unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_dir(&directory.0).unwrap();
        view.select_browser_source(BrowserSection::Place(directory.0.clone()), cx);
        assert!(view.browser.loading);
        assert!(
            view.browser_visible_entries(cx).is_empty(),
            "same-root reselect exposes retained offline listing before availability returns"
        );
        view.browser
            .search
            .update(cx, |input, cx| input.set_text("loop", cx));
        assert!(
            view.browser_visible_entries(cx).is_empty(),
            "query exposes cached offline index before availability returns"
        );
        let generation = view.browser.playback_generation;
        // A closure painted before reselect may still carry the former path.
        view.play_browser_sample_from(path.clone(), 0.5, cx);
        assert_eq!(
            view.browser.playback_generation, generation,
            "old waveform callback admitted stale preview"
        );
        view.insert_browser_audio(path.clone(), cx);
        assert!(
            view.last_enqueued_import_to_track.is_none(),
            "old result callback admitted stale import"
        );
        let (current, fresh) = view.browser.scans.complete(old.generation);
        assert!(!current);
        let fresh = fresh.unwrap();
        view.browser_scan_finished(fresh, hex_library::scan_directory(&directory.0), cx);
        assert!(!view.browser.loading);
        assert!(view.browser.error.is_some());
        assert!(view.browser_visible_entries(cx).is_empty());
        let generation = view.browser.playback_generation;
        view.play_browser_sample_from(path, 0.5, cx);
        assert_eq!(view.browser.playback_generation, generation);
        cx.notify();
    });
}

#[gpui::test]
fn place_correctness_refresh_retains_identity_only_after_current_success(cx: &mut TestAppContext) {
    let (handle, editor, _, directory, path) = browser_drag_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    editor.update(&mut visual, |view, cx| {
        view.browser.section = BrowserSection::Place(directory.0.clone());
        view.browser.loading = false;
        view.browser.error = None;
        view.browser
            .search
            .update(cx, |input, cx| input.set_text("loop", cx));
        view.browser.results_focused = true;
        view.browser.selected_index = 0;
        view.browser.scans = Default::default();
        let old = view.browser.scans.request(directory.0.clone()).unwrap();
        view.request_browser_scan(directory.0.clone(), cx);
        assert!(view.browser_visible_entries(cx).is_empty());
        // A concurrent catalog refresh must not discard the privately saved identity.
        std::fs::write(directory.0.join("another.wav"), mono_pcm_wav(16)).unwrap();
        view.apply_sample_catalog(
            hex_library::catalog::scan_samples(std::slice::from_ref(&directory.0)).catalog,
            cx,
        );
        // The superseded worker must not reauthorize rows.
        let (current, fresh) = view.browser.scans.complete(old.generation);
        assert!(!current);
        assert!(view.browser_visible_entries(cx).is_empty());
        view.browser_scan_finished(
            fresh.unwrap(),
            hex_library::scan_directory(&directory.0),
            cx,
        );
        assert!(!view.browser.loading);
        assert_eq!(view.browser_visible_entries(cx).len(), 1);
        assert!(view.browser.results_focused);
        assert_eq!(
            view.browser_visible_entries(cx)[view.browser.selected_index].path,
            path
        );
        // An available, freshly validated row still admits manual offset preview.
        let generation = view.browser.playback_generation;
        view.play_browser_sample_from(path, 0.5, cx);
        assert_eq!(view.browser.playback_generation, generation + 1);
        cx.notify();
    });
}

#[gpui::test]
fn browser_tree_refresh_updates_expanded_branch_hidden_by_indexed_query(cx: &mut TestAppContext) {
    let (handle, editor, _, directory, visible_sample) = browser_drag_fixture(cx);
    let branch = directory.0.join("branch");
    std::fs::create_dir(&branch).unwrap();
    let old = branch.join("old.wav");
    let fresh = branch.join("fresh.wav");
    std::fs::write(&old, mono_pcm_wav(16)).unwrap();
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    editor.update(&mut visual, |view, cx| {
        view.browser.auto_audition = false;
        view.select_browser_source(BrowserSection::Samples, cx);
        cx.notify();
    });
    visual.run_until_parked();
    for row in [0, 1] {
        let selector = Box::leak(format!("browser-disclosure-{row}").into_boxed_str());
        let disclosure = visual.debug_bounds(selector).unwrap();
        visual.simulate_mouse_down(disclosure.center(), MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(disclosure.center(), MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
    }
    let before = editor.read_with(&visual, |view, cx| {
        assert!(
            view.browser_visible_entries(cx)
                .iter()
                .any(|entry| entry.path == old)
        );
        view.browser.tree.requests_started
    });
    editor.update(&mut visual, |view, cx| {
        view.browser.search.update(cx, |input, cx| {
            input.set_text("loop", cx);
            cx.notify();
        });
    });
    visual.run_until_parked();
    editor.read_with(&visual, |view, cx| {
        let entries = view.browser_visible_entries(cx);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, visible_sample);
    });
    std::fs::remove_file(&old).unwrap();
    std::fs::write(&fresh, mono_pcm_wav(32)).unwrap();
    editor.update(&mut visual, |view, cx| view.refresh_browser_tree(cx));
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert!(
            view.browser
                .samples
                .catalog
                .entries()
                .iter()
                .any(|entry| entry.path == fresh)
        );
        println!(
            "hidden-branch refresh started {} folder requests",
            view.browser.tree.requests_started - before
        );
    });
    editor.update(&mut visual, |view, cx| {
        view.browser.search.update(cx, |input, cx| input.clear(cx));
    });
    visual.run_until_parked();
    assert!(
        visual.debug_bounds("browser-entry-label-2").is_some(),
        "expanded child row must be rendered after clearing query"
    );
    editor.read_with(&visual, |view, cx| {
        let entries = view.browser_visible_entries(cx);
        assert!(
            entries.iter().any(|entry| entry.path == fresh),
            "clearing query restored stale cached contents despite refresh"
        );
        assert!(!entries.iter().any(|entry| entry.path == old));
        assert_eq!(
            view.browser.tree.requests_started - before,
            2,
            "only the two already-expanded folders need enumeration"
        );
        assert!(view.pending_audio_commands.is_empty());
        assert!(view.last_enqueued_import_to_track.is_none());
    });
}

#[gpui::test]
fn browser_place_icon_popup_outside_click_does_not_start_arrangement_gesture(
    cx: &mut TestAppContext,
) {
    cx.update(super::super::bind_editor_keys);
    let (handle, editor, _, _directory, _) = browser_drag_fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    editor.update(visual, |view, _| {
        view.browser.preferences_loading = false;
        view.browser.preferences_writable = true;
    });
    let row = visual.debug_bounds("browser-place-0").unwrap();
    visual.simulate_mouse_down(row.center(), MouseButton::Right, Modifiers::default());
    visual.simulate_mouse_up(row.center(), MouseButton::Right, Modifiers::default());
    assert!(visual.debug_bounds("context-menu-panel").is_some());
    let lane = visual.debug_bounds("main-track-lane").unwrap();
    visual.simulate_mouse_down(lane.center(), MouseButton::Left, Modifiers::default());
    editor.read_with(visual, |view, _| {
        assert!(
            view.time_selection_gesture.is_none(),
            "outside click through icon picker started arrangement selection"
        );
        assert!(view.time_range_move_gesture.is_none());
    });
    visual.simulate_mouse_up(lane.center(), MouseButton::Left, Modifiers::default());
}

fn place_icon_preferences_window(
    cx: &mut TestAppContext,
    directory: &PreferencesTestDirectory,
    config: &std::path::Path,
    recent: &std::path::Path,
) -> (gpui::AnyWindowHandle, gpui::Entity<EditorView>) {
    let home = directory.0.join("icon-home");
    let music = home.join("Music");
    std::fs::create_dir_all(&music).unwrap();
    std::fs::write(music.join("fixture.wav"), mono_pcm_wav(24)).unwrap();
    let bootstrap = cx.add_window(|_, _| Empty);
    let handle = *bootstrap;
    let editor = cx
        .update_window(handle, |_, window, cx| {
            let service = Service::new(48_000, 32, 8_000_000).unwrap();
            window.replace_root(cx, |window, cx| {
                EditorView::new_with_browser_paths(
                    window,
                    cx,
                    service,
                    Err("offline icon test".into()),
                    (
                        config.to_owned(),
                        recent.to_owned(),
                        directory.0.join("icon-theme.json"),
                        directory.0.join("project-directory.json"),
                    ),
                    None,
                    crate::editor::test_environment::Inputs {
                        home: Some(home),
                        ..Default::default()
                    },
                )
            })
        })
        .unwrap();
    (handle, editor)
}

#[gpui::test]
fn browser_place_icons_input_persistence_identity_fallback_and_geometry(cx: &mut TestAppContext) {
    cx.update(super::super::bind_editor_keys);
    let directory = PreferencesTestDirectory::new();
    let config = directory.0.join("browser.json");
    let recent = directory.0.join("recent.json");
    let first = directory.0.join("one/same");
    let second = directory.0.join("two/same");
    let unknown = directory.0.join("unknown");
    let mut saved =
        hex_preferences::BrowserPlaces::new([first.clone(), second.clone(), unknown.clone()])
            .unwrap();
    saved.set_icon(&unknown, Some("future-icon")).unwrap();
    hex_preferences::save(&config, &saved).unwrap();
    let (handle, editor) = place_icon_preferences_window(cx, &directory, &config, &recent);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    editor.read_with(visual, |view, _| {
        assert_eq!(
            view.browser_place_icon_path(&first),
            "icons/browser-place/folder.svg"
        );
        assert_eq!(
            view.browser_place_icon_path(&unknown),
            "icons/browser-place/folder.svg"
        );
        assert!(
            view.browser
                .places_status
                .as_deref()
                .unwrap()
                .contains("icons unavailable")
        );
    });
    visual.simulate_keystrokes("cmd-f");
    let row = visual.debug_bounds("browser-place-2").unwrap();
    let generation = editor.read_with(visual, |view, _| view.browser.playback_generation);
    visual.simulate_mouse_down(row.center(), MouseButton::Right, Modifiers::default());
    visual.simulate_mouse_up(row.center(), MouseButton::Right, Modifiers::default());
    let popup = visual.debug_bounds("context-menu-panel").unwrap();
    visual.update(|window, _| {
        assert!(popup.left() >= px(0.0) && popup.top() >= px(0.0));
        assert!(popup.right() <= window.bounds().size.width);
        assert!(popup.bottom() <= window.bounds().size.height);
    });
    visual.simulate_keystrokes("space cmd-shift-t right right enter");
    visual.run_until_parked();
    visual.update(|window, cx| {
        editor.read_with(cx, |view, cx| {
            assert!(view.context_menu.is_none());
            assert!(
                view.browser
                    .search
                    .read(cx)
                    .focus_handle()
                    .is_focused(window)
            );
            assert_eq!(
                view.browser_place_icon_path(&first),
                "icons/browser-place/samples.svg"
            );
            assert_eq!(view.browser.section, BrowserSection::All);
            assert_eq!(view.browser.playback_generation, generation);
            assert!(view.pending_requests.is_empty());
            assert!(view.pending_audio_commands.is_empty());
        })
    });
    // A pointer choice for the same glyph is allowed on another, same-named Place.
    let row = visual.debug_bounds("browser-place-3").unwrap();
    visual.simulate_mouse_down(row.center(), MouseButton::Right, Modifiers::default());
    visual.simulate_mouse_up(row.center(), MouseButton::Right, Modifiers::default());
    let sample = visual.debug_bounds("context-menu-choice-2").unwrap();
    visual.simulate_click(sample.center(), Modifiers::default());
    visual.run_until_parked();
    let saved = hex_preferences::load(&config).unwrap();
    assert_eq!(saved.icon(&first), Some("samples"));
    assert_eq!(saved.icon(&second), Some("samples"));
    assert_eq!(saved.icon(&unknown), Some("future-icon"));
    // Reordering rows never retargets assignment; unknown IDs remain recoverable.
    editor.update(visual, |view, cx| {
        view.browser.places.swap(2, 4);
        cx.notify();
    });
    let divider = visual.debug_bounds("browser-sidebar-resize").unwrap();
    let sidebar = visual.debug_bounds("browser-sidebar").unwrap();
    let target = divider.center() + point(px(23.0) - sidebar.size.width, px(0.0));
    visual.simulate_mouse_down(divider.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let control = visual.debug_bounds("browser-control-⚙").unwrap();
    for (index, expected) in [
        (2, "icons/browser-place/folder.svg"),
        (3, "icons/browser-place/samples.svg"),
        (4, "icons/browser-place/samples.svg"),
    ] {
        let row = visual
            .debug_bounds(Box::leak(format!("browser-place-{index}").into_boxed_str()))
            .unwrap();
        let glyph = visual
            .debug_bounds(Box::leak(
                format!("browser-picker-glyph-place-{index}").into_boxed_str(),
            ))
            .unwrap();
        assert_eq!(glyph.center().x, control.center().x);
        assert!(glyph.left() >= row.left() && glyph.right() <= row.right());
        editor.read_with(visual, |view, _| {
            assert_eq!(
                view.browser_place_icon_path(&view.browser.places[index]),
                expected
            )
        });
    }
    // Escape dismisses without choosing; normal left-click still selects this Place.
    let row = visual.debug_bounds("browser-place-4").unwrap();
    visual.simulate_mouse_down(row.center(), MouseButton::Right, Modifiers::default());
    visual.simulate_mouse_up(row.center(), MouseButton::Right, Modifiers::default());
    visual.simulate_keystrokes("down escape");
    assert_eq!(
        hex_preferences::load(&config).unwrap().icon(&first),
        Some("samples")
    );
    visual.simulate_click(row.center(), Modifiers::default());
    editor.read_with(visual, |view, _| {
        assert_eq!(view.browser.section, BrowserSection::Place(first.clone()))
    });
    // Recreate the actual editor/preferences load path, using only disposable test files.
    let (reopened_handle, reopened) =
        place_icon_preferences_window(cx, &directory, &config, &recent);
    let reopened_visual = &mut VisualTestContext::from_window(reopened_handle, cx);
    reopened_visual.run_until_parked();
    reopened.read_with(reopened_visual, |view, _| {
        assert_eq!(
            view.browser_place_icon_path(&first),
            "icons/browser-place/samples.svg"
        );
        assert_eq!(
            view.browser_place_icon_path(&second),
            "icons/browser-place/samples.svg"
        );
        assert_eq!(
            view.browser_place_icon_path(&unknown),
            "icons/browser-place/folder.svg"
        );
        assert_eq!(
            view.browser.place_icons.get(&unknown).map(String::as_str),
            Some("future-icon")
        );
    });
}

#[gpui::test]
fn browser_place_icons_coalesced_save_errors_and_stale_menu_are_bounded(cx: &mut TestAppContext) {
    cx.update(super::super::bind_editor_keys);
    let directory = PreferencesTestDirectory::new();
    let config = directory.0.join("browser.json");
    let path = directory.0.join("added");
    let saved = hex_preferences::BrowserPlaces::new([path.clone()]).unwrap();
    hex_preferences::save(&config, &saved).unwrap();
    let (handle, editor) =
        place_icon_preferences_window(cx, &directory, &config, &directory.0.join("recent.json"));
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    editor.update(visual, |view, cx| {
        for id in ["samples", "waveform", "projects"] {
            view.set_browser_place_icon(path.clone(), Some(id), cx);
        }
    });
    visual.run_until_parked();
    assert_eq!(
        hex_preferences::load(&config).unwrap().icon(&path),
        Some("projects")
    );
    let valid_bytes = std::fs::read(&config).unwrap();
    // A changed corrupt file cannot be overwritten by a later icon choice.
    let corrupt = format!(
        "{{\"version\":1,\"additional_places\":[],\"{}\":0}}",
        "bad".repeat(200)
    )
    .into_bytes();
    std::fs::write(&config, &corrupt).unwrap();
    let row = visual.debug_bounds("browser-place-2").unwrap();
    visual.simulate_mouse_down(row.center(), MouseButton::Right, Modifiers::default());
    visual.simulate_mouse_up(row.center(), MouseButton::Right, Modifiers::default());
    visual.simulate_keystrokes("right enter");
    visual.run_until_parked();
    editor.read_with(visual, |view, _| {
        assert!(
            view.browser
                .places_status
                .as_deref()
                .unwrap()
                .contains("could not save")
        )
    });
    assert_eq!(std::fs::read(&config).unwrap(), corrupt);
    // Explicitly choosing the same unsaved icon retries after the external failure is repaired.
    std::fs::write(&config, &valid_bytes).unwrap();
    visual.simulate_mouse_down(row.center(), MouseButton::Right, Modifiers::default());
    visual.simulate_mouse_up(row.center(), MouseButton::Right, Modifiers::default());
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    assert_eq!(
        hex_preferences::load(&config).unwrap().icon(&path),
        Some("collection")
    );
    // Removed identity invalidates the existing menu before a queued choice can write.
    visual.simulate_mouse_down(row.center(), MouseButton::Right, Modifiers::default());
    visual.simulate_mouse_up(row.center(), MouseButton::Right, Modifiers::default());
    editor.update(visual, |view, cx| {
        view.browser.places.retain(|place| place != &path);
        cx.notify();
    });
    visual.run_until_parked();
    editor.read_with(visual, |view, _| assert!(view.context_menu.is_none()));
    std::fs::write(&config, &corrupt).unwrap();
    let (other_handle, other) = place_icon_preferences_window(
        cx,
        &directory,
        &config,
        &directory.0.join("other-recent.json"),
    );
    let other_visual = &mut VisualTestContext::from_window(other_handle, cx);
    other_visual.run_until_parked();
    let status_row = other_visual.debug_bounds("browser-places-status").unwrap();
    other.update(other_visual, |view, cx| {
        view.browser.places_status = Some("could not load browser places".into());
        cx.notify();
    });
    let short_status = other_visual.debug_bounds("browser-places-status").unwrap();
    assert_eq!(
        status_row.size.height, short_status.size.height,
        "long errors must retain the actual theme's single-line status height"
    );
    // Reload the actual bounded load error for the remaining state assertions.
    other.update(other_visual, |view, cx| {
        view.browser_places_loaded(hex_preferences::load(&config), cx)
    });
    other.read_with(other_visual, |view, _| {
        assert!(
            view.browser
                .places_status
                .as_deref()
                .unwrap()
                .chars()
                .count()
                <= 200
        );
        assert!(!view.browser.preferences_writable);
        assert!(
            view.browser
                .places_status
                .as_deref()
                .unwrap()
                .contains("could not load")
        );
    });
    other_visual.update(|window, cx| {
        other.update(cx, |view, cx| {
            let custom = directory.0.join("unsaved");
            view.browser.places.push(custom.clone());
            view.open_browser_place_icons(custom, point(px(50.0), px(50.0)), window, cx);
            assert!(view.context_menu.is_none());
        })
    });
    assert_eq!(std::fs::read(&config).unwrap(), corrupt);
}

#[gpui::test]
fn browser_place_icons_catalog_uses_same_bounds_expanded_and_collapsed(cx: &mut TestAppContext) {
    cx.update(super::super::bind_editor_keys);
    let (handle, editor, _, directory, _) = browser_drag_fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    editor.update(visual, |view, cx| {
        view.browser.preferences_path = directory.0.join("icon-preferences.json");
        view.browser.preferences_loading = false;
        view.queue_browser_places_save(cx);
    });
    visual.run_until_parked();
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            for path in [view.browser.home.clone(), view.browser.music.clone()] {
                view.browser
                    .place_icons
                    .insert(path.clone(), "samples".into());
                assert_eq!(
                    view.browser_place_icon_path(&path),
                    "icons/browser-place/folder.svg"
                );
                view.open_browser_place_icons(path, point(px(0.0), px(0.0)), window, cx);
                assert!(
                    view.context_menu.is_none(),
                    "these built-ins are absent from this custom-only fixture"
                );
            }
            view.browser.preferences_loading = true;
            view.open_browser_place_icons(directory.0.clone(), point(px(0.0), px(0.0)), window, cx);
            assert!(
                view.context_menu.is_none(),
                "no icon choice may race startup load"
            );
            view.browser.preferences_loading = false;
        })
    });
    for width in [124.0, 23.0] {
        let sidebar = visual.debug_bounds("browser-sidebar").unwrap();
        let divider = visual.debug_bounds("browser-sidebar-resize").unwrap();
        let target = divider.center() + point(px(width) - sidebar.size.width, px(0.0));
        visual.simulate_mouse_down(divider.center(), MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
        for (index, (id, glyph)) in [
            (None, "icons/browser-place/folder.svg"),
            (Some("samples"), "icons/browser-place/samples.svg"),
            (Some("devices"), "icons/browser-place/devices.svg"),
            (Some("projects"), "icons/browser-place/projects.svg"),
            (Some("collection"), "icons/browser-place/collection.svg"),
            (Some("waveform"), "icons/browser-place/waveform.svg"),
            (Some("search"), "icons/browser-place/search.svg"),
        ]
        .into_iter()
        .enumerate()
        {
            let row = visual.debug_bounds("browser-place-0").unwrap();
            visual.simulate_mouse_down(row.center(), MouseButton::Right, Modifiers::default());
            visual.simulate_mouse_up(row.center(), MouseButton::Right, Modifiers::default());
            let choice_index = if index == 0 { 0 } else { index + 1 };
            let cell = visual
                .debug_bounds(Box::leak(
                    format!("context-menu-choice-{choice_index}").into_boxed_str(),
                ))
                .unwrap();
            visual.simulate_click(cell.center(), Modifiers::default());
            visual.run_until_parked();
            let bounds = visual.debug_bounds("browser-picker-glyph-place-0").unwrap();
            let control = visual.debug_bounds("browser-control-⚙").unwrap();
            assert_eq!(bounds.center().x, control.center().x);
            assert!(bounds.left() >= row.left() && bounds.right() <= row.right());
            assert!(f32::from(bounds.center().y - row.center().y).abs() <= 0.5);
            editor.read_with(visual, |view, _| {
                assert_eq!(view.browser_place_icon_path(&directory.0), glyph)
            });
            assert_eq!(
                hex_preferences::load(&directory.0.join("icon-preferences.json"))
                    .unwrap()
                    .icon(&directory.0),
                id
            );
        }
    }
}

#[gpui::test]
fn browser_place_icons_preference_fixture_bounds_startup_before_workers(cx: &mut TestAppContext) {
    let directory = PreferencesTestDirectory::new();
    let config = directory.0.join("missing-browser.json");
    let recent = directory.0.join("recent.json");
    let (handle, editor) = place_icon_preferences_window(cx, &directory, &config, &recent);
    // Check before parking/running any newly scheduled filesystem work.
    editor.read_with(cx, |view, _| {
        for path in [
            &view.browser.home,
            &view.browser.music,
            &view.browser.directory,
        ] {
            assert!(
                path.starts_with(&directory.0),
                "startup browser root escaped private fixture: {path:?}"
            );
        }
        assert_eq!(
            view.browser.places,
            vec![view.browser.music.clone(), view.browser.home.clone()]
        );
        assert_eq!(view.browser.preferences_path, config);
    });
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    editor.read_with(visual, |view, _| {
        assert!(!view.browser.loading);
        assert!(!view.browser.preferences_loading);
        assert_eq!(view.browser.samples.catalog.entries().len(), 1);
        assert!(
            view.sample_roots()
                .iter()
                .all(|path| path.starts_with(&directory.0))
        );
        assert_eq!(view.browser.listing.as_ref().unwrap().entries.len(), 1);
        assert_eq!(
            view.browser.listing.as_ref().unwrap().entries[0].path,
            view.browser.music.join("fixture.wav")
        );
    });
    assert!(config.with_extension("samples-v1.db").is_file());
    assert!(
        !config.exists(),
        "missing explicit preferences must not be manufactured"
    );
}

#[path = "browser_search_selection_ui_test.rs"]
mod search_selection;

#[gpui::test]
fn browser_place_icon_grid_uses_clicked_row_reset_and_reopens(cx: &mut TestAppContext) {
    cx.update(super::super::bind_editor_keys);
    let directory = PreferencesTestDirectory::new();
    let config = directory.0.join("grid-browser.json");
    let recent = directory.0.join("recent.json");
    let first = directory.0.join("one/same");
    let second = directory.0.join("two/same");
    hex_preferences::save(
        &config,
        &hex_preferences::BrowserPlaces::new([first.clone(), second.clone()]).unwrap(),
    )
    .unwrap();
    let (handle, editor) = place_icon_preferences_window(cx, &directory, &config, &recent);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    visual.simulate_keystrokes("cmd-f");
    for (row_index, choice, expected, width) in [
        (2, 2, Some("samples"), 124.0),
        (3, 6, Some("waveform"), 23.0),
        (2, 0, None, 23.0),
    ] {
        let sidebar = visual.debug_bounds("browser-sidebar").unwrap();
        let divider = visual.debug_bounds("browser-sidebar-resize").unwrap();
        let target = divider.center() + point(px(width) - sidebar.size.width, px(0.0));
        visual.simulate_mouse_down(divider.center(), MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
        let (source, generation) = editor.read_with(visual, |view, _| {
            (
                view.browser.section.clone(),
                view.browser.playback_generation,
            )
        });
        let row = visual
            .debug_bounds(Box::leak(
                format!("browser-place-{row_index}").into_boxed_str(),
            ))
            .unwrap();
        // Two positions inside the same row must produce the same row anchor.
        for fraction in [0.2, 0.8] {
            let hit = point(row.left() + row.size.width * fraction, row.center().y);
            visual.simulate_mouse_down(hit, MouseButton::Right, Modifiers::default());
            visual.simulate_mouse_up(hit, MouseButton::Right, Modifiers::default());
            let grid = visual
                .debug_bounds("context-menu-icon-grid")
                .expect("actual row context menu must contain a compact icon grid");
            let first_cell = visual.debug_bounds("context-menu-choice-1").unwrap();
            let second_cell = visual.debug_bounds("context-menu-choice-2").unwrap();
            let next_row = visual.debug_bounds("context-menu-choice-5").unwrap();
            assert_eq!(first_cell.top(), second_cell.top());
            assert_eq!(first_cell.left(), next_row.left());
            assert!(next_row.top() > first_cell.top());
            assert!(grid.size.width <= px(120.0));
            let panel = visual.debug_bounds("context-menu-panel").unwrap();
            assert_eq!(panel.left(), row.left().max(px(4.0)));
            assert_eq!(panel.top(), row.bottom());
            visual.simulate_keystrokes("escape");
        }
        visual.simulate_keystrokes("cmd-f");
        visual.simulate_mouse_down(row.center(), MouseButton::Right, Modifiers::default());
        visual.simulate_mouse_up(row.center(), MouseButton::Right, Modifiers::default());
        let current = editor.read_with(visual, |view, _| {
            view.context_menu
                .as_ref()
                .unwrap()
                .items
                .iter()
                .position(|item| item.checked)
                .unwrap()
        });
        assert!(
            visual
                .debug_bounds(Box::leak(
                    format!("context-menu-current-{current}").into_boxed_str()
                ))
                .is_some()
        );
        let cell = visual
            .debug_bounds(Box::leak(
                format!("context-menu-choice-{choice}").into_boxed_str(),
            ))
            .unwrap();
        if row_index == 3 {
            visual.simulate_keystrokes("right right down enter");
        } else {
            visual.simulate_click(cell.center(), Modifiers::default());
        }
        visual.run_until_parked();
        let path = if row_index == 2 { &first } else { &second };
        assert_eq!(hex_preferences::load(&config).unwrap().icon(path), expected);
        visual.update(|window, cx| {
            editor.read_with(cx, |view, cx| {
                assert!(view.context_menu.is_none());
                assert_eq!(view.browser.section, source);
                assert_eq!(view.browser.playback_generation, generation);
                assert!(view.pending_requests.is_empty() && view.pending_audio_commands.is_empty());
                assert!(
                    view.browser
                        .search
                        .read(cx)
                        .focus_handle()
                        .is_focused(window),
                    "restore actual previous search focus, do not invent entry"
                );
            })
        });
    }
    // A menu entered from non-text context restores that owner, not search.
    visual.update(|window, cx| editor.read_with(cx, |view, _| view.focus.focus(window)));
    let row = visual.debug_bounds("browser-place-3").unwrap();
    visual.simulate_mouse_down(row.center(), MouseButton::Right, Modifiers::default());
    visual.simulate_mouse_up(row.center(), MouseButton::Right, Modifiers::default());
    visual.simulate_keystrokes("left up escape");
    visual.update(|window, cx| {
        editor.read_with(cx, |view, cx| {
            assert!(view.focus.is_focused(window));
            assert!(
                !view
                    .browser
                    .search
                    .read(cx)
                    .focus_handle()
                    .is_focused(window)
            );
        })
    });
    assert_eq!(
        hex_preferences::load(&config).unwrap().icon(&second),
        Some("waveform")
    );
    let (handle, reopened) = place_icon_preferences_window(cx, &directory, &config, &recent);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    reopened.read_with(visual, |view, _| {
        assert_eq!(
            view.browser_place_icon_path(&first),
            "icons/browser-place/folder.svg"
        );
        assert_eq!(
            view.browser_place_icon_path(&second),
            "icons/browser-place/waveform.svg"
        );
    });
}

#[path = "browser_focus_ui_test.rs"]
mod explicit_focus;

#[path = "browser_insert_ui_test.rs"]
mod insertion;
#[gpui::test]
fn browser_picker_compact_gear_closes_without_expanding_then_opens_explicitly(
    cx: &mut TestAppContext,
) {
    let (handle, editor, _, _directory, _) = browser_drag_fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let closed_top = visual.debug_bounds("browser-place-0").unwrap().top();
    let gear = visual.debug_bounds("browser-control-⚙").unwrap();
    visual.simulate_click(gear.center(), Modifiers::default());
    let open_top = visual.debug_bounds("browser-place-0").unwrap().top();
    assert!(
        open_top > closed_top,
        "actual settings rows must be present"
    );
    let sidebar = visual.debug_bounds("browser-sidebar").unwrap();
    let divider = visual.debug_bounds("browser-sidebar-resize").unwrap();
    let target = divider.center() + point(px(23.0) - sidebar.size.width, px(0.0));
    visual.simulate_mouse_down(divider.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert_eq!(
        visual.debug_bounds("browser-place-0").unwrap().top(),
        open_top
    );
    let gear = visual.debug_bounds("browser-control-⚙").unwrap();
    visual.simulate_click(gear.center(), Modifiers::default());
    assert_eq!(
        visual.debug_bounds("browser-sidebar").unwrap().size.width,
        px(23.0),
        "closing compact settings must not widen the picker"
    );
    assert_eq!(
        visual.debug_bounds("browser-place-0").unwrap().top(),
        closed_top,
        "gear must remove the actual open settings rows"
    );
    // The existing deliberate compact-open behavior still expands usable settings.
    let gear = visual.debug_bounds("browser-control-⚙").unwrap();
    visual.simulate_click(gear.center(), Modifiers::default());
    assert_eq!(
        visual.debug_bounds("browser-sidebar").unwrap().size.width,
        px(124.0)
    );
    assert_eq!(
        visual.debug_bounds("browser-place-0").unwrap().top(),
        open_top
    );
    let gear = visual.debug_bounds("browser-control-⚙").unwrap();
    visual.simulate_click(gear.center(), Modifiers::default());
    assert_eq!(
        visual.debug_bounds("browser-place-0").unwrap().top(),
        closed_top
    );
    editor.read_with(visual, |view, _| {
        assert!(view.pending_requests.is_empty());
        assert!(view.pending_audio_commands.is_empty());
    });
}

#[path = "browser_history_ui_test.rs"]
mod browser_history_ui_test;
#[path = "browser_builtin_source_ui_test.rs"]
mod builtin_sources;
#[path = "browser_resize_feedback_ui_test.rs"]
mod resize_feedback;

#[path = "browser_tag_editor_geometry_ui_test.rs"]
mod tag_editor_geometry;

#[path = "editor_test_environment_ui_test.rs"]
mod test_environment;

#[path = "browser_picker_compact_ui_test.rs"]
mod compact_picker;

#[path = "browser_arrow_fixture_ui_test.rs"]
mod arrow_fixture;
use arrow_fixture::arrow_row_fixture;

fn unhighlighted_arrow_action(cx: &mut TestAppContext, sample: bool, escape: bool, left: bool) {
    let (handle, editor, _directory, samples) = arrow_row_fixture(cx, sample);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let category = if sample {
        "browser-place-0"
    } else {
        "browser-section-projects"
    };
    let point = visual.debug_bounds(category).unwrap().center();
    visual.simulate_click(point, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |v, cx| {
        let entries = v.browser_visible_entries(cx);
        assert!(!entries.is_empty(), "actual async entries available");
        assert_eq!(
            entries[0].path,
            if sample {
                samples.join("loop.wav")
            } else {
                samples.join("a")
            }
        );
    });
    if escape || left {
        let row = visual.debug_bounds("browser-entry-0").unwrap().center();
        visual.simulate_click(row, Modifiers::default());
        editor.read_with(&visual, |v, _| assert!(v.browser.results_focused));
        if left {
            visual.simulate_keystrokes("right");
            visual.run_until_parked();
            editor.read_with(&visual, |v, _| {
                assert!(
                    (v.browser.tree.disclosure(&samples.join("a"))
                        == super::tree::Disclosure::Expanded)
                )
            });
        }
        visual.simulate_keystrokes("escape");
    }
    visual.update(|window, cx| {
        editor.read_with(cx, |v, cx| {
            assert!(!v.browser.results_focused);
            assert!(v.browser.results_focus.is_focused(window));
            assert_eq!(v.browser_key_context(window, cx), "hex_browser");
        })
    });
    let canonical = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().clone());
    let before = editor.read_with(&visual, |v, _| {
        (
            v.browser.playback_generation,
            v.browser.tree.disclosure(&samples.join("a")),
            v.browser.tree.requests_started,
        )
    });
    visual.simulate_keystrokes(if left { "left" } else { "right" });
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        let after = (v.browser.playback_generation, v.browser.tree.disclosure(&samples.join("a")), v.browser.tree.requests_started);
        eprintln!("ACTUAL_UNHIGHLIGHTED_ROW sample={sample} escape={escape} left={left} before={before:?} after={after:?}");
        assert_eq!(after, before, "unhighlighted navigation must not dispatch a row action");
        assert!(v.pending_requests.is_empty() && v.pending_audio_commands.is_empty());
        let state = v.state.as_ref().unwrap();
        assert_eq!(state.project, canonical.project);
        assert_eq!((state.revision, state.can_undo, state.can_redo, state.unsaved),
                   (canonical.revision, canonical.can_undo, canonical.can_redo, canonical.unsaved));
        assert_eq!(v.worker_requests_pending, 0);
    });
}

#[gpui::test]
fn browser_arrows_source_sample_right(cx: &mut TestAppContext) {
    unhighlighted_arrow_action(cx, true, false, false);
}
#[gpui::test]
fn browser_arrows_escape_sample_right(cx: &mut TestAppContext) {
    unhighlighted_arrow_action(cx, true, true, false);
}
#[gpui::test]
fn browser_arrows_source_folder_right(cx: &mut TestAppContext) {
    unhighlighted_arrow_action(cx, false, false, false);
}
#[gpui::test]
fn browser_arrows_escape_folder_right(cx: &mut TestAppContext) {
    unhighlighted_arrow_action(cx, false, true, false);
}
#[gpui::test]
fn browser_arrows_escape_folder_left(cx: &mut TestAppContext) {
    unhighlighted_arrow_action(cx, false, true, true);
}

#[gpui::test]
fn browser_arrows_selected_controls(cx: &mut TestAppContext) {
    for sample in [false, true] {
        let (handle, editor, _directory, samples) = arrow_row_fixture(cx, sample);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let category = if sample {
            "browser-place-0"
        } else {
            "browser-section-projects"
        };
        let category = visual.debug_bounds(category).unwrap().center();
        visual.simulate_click(category, Modifiers::default());
        visual.run_until_parked();
        let row = visual.debug_bounds("browser-entry-0").unwrap().center();
        visual.simulate_click(row, Modifiers::default());
        let before = editor.read_with(&visual, |v, _| {
            assert!(v.browser.results_focused);
            v.browser.playback_generation
        });
        visual.simulate_keystrokes("right");
        visual.run_until_parked();
        editor.read_with(&visual, |v, _| {
            if sample {
                assert!(v.browser.playback_generation > before);
            } else {
                assert_eq!(
                    v.browser.tree.disclosure(&samples.join("a")),
                    super::tree::Disclosure::Expanded
                );
            }
        });
        if !sample {
            visual.simulate_keystrokes("left");
            editor.read_with(&visual, |v, _| {
                assert_ne!(
                    v.browser.tree.disclosure(&samples.join("a")),
                    super::tree::Disclosure::Expanded
                )
            });
        }
    }
}

#[gpui::test]
fn browser_arrows_up_down_enter_results_and_command_f_keeps_text_input(cx: &mut TestAppContext) {
    use gpui::EntityInputHandler;
    let (handle, editor, _directory, _samples) = arrow_row_fixture(cx, true);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let place = visual.debug_bounds("browser-place-0").unwrap().center();
    visual.simulate_click(place, Modifiers::default());
    visual.run_until_parked();
    for key in ["down", "up"] {
        editor.read_with(&visual, |v, _| assert!(!v.browser.results_focused));
        visual.simulate_keystrokes(key);
        let before = editor.read_with(&visual, |v, _| {
            assert!(v.browser.results_focused);
            assert_eq!(v.browser.selected_index, 0);
            v.browser.playback_generation
        });
        visual.simulate_keystrokes("right");
        editor.read_with(&visual, |v, _| {
            assert!(v.browser.playback_generation > before)
        });
        visual.simulate_keystrokes("escape");
    }
    visual.simulate_keystrokes("cmd-f");
    visual.simulate_input("loop");
    visual.simulate_keystrokes("cmd-f");
    let before = editor.read_with(&visual, |v, _| v.browser.playback_generation);
    visual.update(|window, cx| {
        let input = editor.read(cx).browser.search.clone();
        input.update(cx, |input, cx| {
            assert_eq!(
                input.selected_text_range(false, window, cx).unwrap().range,
                0..4
            )
        });
    });
    visual.simulate_keystrokes("right");
    visual.simulate_input("x");
    editor.read_with(&visual, |v, cx| {
        assert_eq!(v.browser.search.read(cx).text(), "loopx");
        assert_eq!(v.browser.playback_generation, before);
        assert!(v.pending_requests.is_empty() && v.pending_audio_commands.is_empty());
    });
}

#[path = "browser_divider_paint_ui_test.rs"]
mod divider_paint;

#[path = "browser_instrument_enter_ui_test.rs"]
mod instrument_enter;

#[path = "browser_sampler_drop_ui_test.rs"]
mod sampler_drop;

#[gpui::test]
fn sampler_menu_captures_stale_target_and_correlates_cancel_without_duplicate_load(
    cx: &mut TestAppContext,
) {
    use super::super::context_menu::EditorContextAction;
    let (handle, editor, tracks, _directory, path) =
        browser_drag_fixture_with_tracks(cx, &["target", "other"]);
    cx.update_window(handle, |_, window, cx| {
        editor.update(cx, |view, cx| {
            let state = view.state.as_ref().unwrap();
            let mut model = Editor::new((*state.project).clone(), 4).unwrap();
            model
                .execute(Command::InsertInstrument {
                    track: tracks[0],
                    state: hex_engine::default_native_device_state_for_class(
                        hex_engine::NativeDeviceClass::Instrument,
                        hex_lattice_contract::DeviceTypeId::from_static("hex.instrument.sampler"),
                    )
                    .unwrap(),
                })
                .unwrap();
            let Created::Clip(clip) = model
                .execute(Command::PlaceMidiClip {
                    track: tracks[0],
                    start: hex_project::BeatTime::ZERO,
                    length: hex_project::BeatTime::ONE,
                })
                .unwrap()
            else {
                panic!("clip")
            };
            model
                .execute(Command::AddMidiNote {
                    clip,
                    start: hex_project::BeatTime::ZERO,
                    duration: hex_project::BeatTime::ONE,
                    pitch: 60,
                    channel: 0,
                    velocity: 127,
                })
                .unwrap();
            view.state = Some(Arc::new(ViewState {
                canonical_audio_source: None,
                comparison_audio: None,
                project: Arc::new(model.project().clone()),
                waveforms: Default::default(),
                source_overviews: Default::default(),
                audio_clip_waveforms: Default::default(),
                midi_preview: Arc::default(),
                gain_preview: state.gain_preview.clone(),
                timeline_frames: state.timeline_frames,
                can_undo: state.can_undo,
                can_redo: state.can_redo,
                revision: state.revision,
                unsaved: state.unsaved,
                document_path: state.document_path.clone(),
            }));
            view.selected_track = Some(tracks[0]);
            view.open_first_sampler_menu(path, point(px(20.0), px(100.0)), window, cx);
            let EditorContextAction::LoadFirstSampler(target) =
                view.context_menu.as_ref().unwrap().items[0].action.clone()
            else {
                panic!("sampler menu action")
            };
            assert_eq!(
                view.context_menu.as_ref().unwrap().items[0].label,
                "load into sampler"
            );
            assert!(
                target
                    .replacement
                    .as_ref()
                    .is_some_and(|(_, state)| hex_engine::is_empty_sampler_state_on_control(state))
            );
            assert!(
                !view.first_sampler_current(),
                "an empty sampler has no loaded audio requirement"
            );
            let generation = view.project_generation;
            let pending = view.worker_requests_pending;
            view.project_generation += 1;
            view.load_first_sampler(target.clone(), cx);
            assert_eq!(
                view.worker_requests_pending, pending,
                "stale menu must not enqueue"
            );
            assert!(!view.first_sampler_pending_matches(1));
            view.project_generation = generation;
            view.load_first_sampler(target.clone(), cx);
            assert!(view.first_sampler_pending_matches(1));
            let pending = view.worker_requests_pending;
            view.selected_track = Some(tracks[1]);
            view.load_first_sampler(target, cx);
            assert_eq!(
                view.worker_requests_pending, pending,
                "double action must not enqueue"
            );
            view.cancel_first_sampler_load(Some(1));
            view.first_sampler_load_complete(
                Some(hex_session::service::SamplerLoadCompletion {
                    request_id: 2,
                    project_generation: generation,
                    track: tracks[0],
                }),
                false,
            );
            assert!(
                view.first_sampler_pending_matches(1),
                "unrelated completion cannot clear intent"
            );
            view.first_sampler_load_complete(
                Some(hex_session::service::SamplerLoadCompletion {
                    request_id: 1,
                    project_generation: generation,
                    track: tracks[0],
                }),
                false,
            );
            assert!(!view.first_sampler_pending_matches(1));
            assert_eq!(view.status, "sampler load cancelled");
            assert_eq!(view.selected_track, Some(tracks[1]));
        });
    })
    .unwrap();
}

#[path = "browser_plugin_ui_test.rs"]
mod plugin_browser;

#[path = "browser_reveal_ui_test.rs"]
mod reveal;

#[gpui::test]
fn browser_file_result_vectors_paint_in_light_and_dark_themes(cx: &mut TestAppContext) {
    let (handle, editor, _, _directory, _) = browser_drag_fixture(cx);
    editor.update(cx, |view, cx| {
        view.browser.open = true;
        view.browser
            .listing
            .as_mut()
            .unwrap()
            .entries
            .push(LibraryEntry {
                name: "project.hex".into(),
                path: view.browser.directory.join("project.hex"),
                kind: EntryKind::Project,
            });
        view.browser.samples.invalidate_listing();
        view.browser.tree.source_changed();
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let bounds = [
        visual.debug_bounds("browser-file-icon-0").unwrap(),
        visual.debug_bounds("browser-file-icon-1").unwrap(),
    ];
    struct RestoreTheme(hex_preferences::ThemePreferences);
    impl Drop for RestoreTheme {
        fn drop(&mut self) {
            crate::theme::apply_preferences(self.0);
        }
    }
    let original = crate::theme::preferences();
    let _restore = RestoreTheme(original);
    for (background, text) in [(0x20, 0xe0), (0xe0, 0x20)] {
        let mut preferences = original;
        preferences.background = hex_preferences::RgbColor::new(background, background, background);
        preferences.text = hex_preferences::RgbColor::new(text, text, text);
        crate::theme::apply_preferences(preferences);
        editor.update(cx, |_, cx| cx.notify());
        visual.run_until_parked();
        let expected = gpui::Hsla::from(gpui::rgb(crate::theme::color(
            crate::theme::PaletteRole::Muted,
        )));
        for (index, selector) in ["browser-file-icon-0", "browser-file-icon-1"]
            .into_iter()
            .enumerate()
        {
            assert_eq!(visual.debug_bounds(selector), Some(bounds[index]));
            assert!(
                visual
                    .rendered_monochrome_colors(selector)
                    .contains(&expected)
            );
        }
    }
}

#[path = "browser_random_ui_test.rs"]
mod random_ui_tests;
#[path = "browser_scrollbar_ui_test.rs"]
mod scrollbar_ui_tests;

#[gpui::test]
fn collections_stay_below_scrollable_sources_in_both_picker_sizes(cx: &mut TestAppContext) {
    let (handle, editor, _, _directory, _) = browser_drag_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    let before = editor.read_with(&visual, |view, _| view.state.as_ref().unwrap().clone());
    visual.run_until_parked();
    let pinned = visual.debug_bounds("browser-collections").unwrap();
    let gear = visual.debug_bounds("browser-control-⚙").unwrap().center();
    visual.simulate_click(gear, Modifiers::default());
    assert_eq!(visual.debug_bounds("browser-collections").unwrap(), pinned);
    for window_size in [(880.0, 540.0), (1440.0, 800.0)] {
        visual.simulate_resize(gpui::size(px(window_size.0), px(window_size.1)));
        for width in [23.0, 124.0] {
            let sources = visual.debug_bounds("browser-sidebar-sources").unwrap();
            visual.simulate_event(gpui::ScrollWheelEvent {
                position: sources.center(),
                delta: gpui::ScrollDelta::Pixels(point(px(0.0), px(500.0))),
                ..Default::default()
            });
            let sidebar = visual.debug_bounds("browser-sidebar").unwrap();
            let divider = visual
                .debug_bounds("browser-sidebar-resize")
                .unwrap()
                .center();
            let target = divider + point(px(width) - sidebar.size.width, px(0.0));
            visual.simulate_mouse_down(divider, MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
            let sidebar = visual.debug_bounds("browser-sidebar").unwrap();
            let sources = visual.debug_bounds("browser-sidebar-sources").unwrap();
            let collections = visual.debug_bounds("browser-collections").unwrap();
            let library = visual
                .debug_bounds("browser-picker-heading-library")
                .unwrap();
            assert_eq!(sidebar.size.width, px(width));
            assert_eq!(library.top(), sidebar.top());
            assert_eq!(sources.bottom(), collections.top());
            assert_eq!(collections.bottom(), sidebar.bottom());
            let first = visual.debug_bounds("browser-collection-0").unwrap();
            assert!(first.top() >= collections.top() && first.bottom() <= collections.bottom());
            visual.simulate_click(first.center(), Modifiers::default());
            visual.update(|window, cx| {
                editor.read_with(cx, |view, _| {
                    assert_eq!(view.browser.section, BrowserSection::Collection(0));
                    assert!(view.browser.results_focus.is_focused(window));
                })
            });
            let all = visual.debug_bounds("browser-section-all").unwrap().center();
            visual.simulate_click(all, Modifiers::default());
            visual.simulate_event(gpui::ScrollWheelEvent {
                position: sources.center(),
                delta: gpui::ScrollDelta::Pixels(point(px(0.0), px(-500.0))),
                ..Default::default()
            });
            assert_eq!(
                visual.debug_bounds("browser-collections").unwrap(),
                collections
            );
            if window_size.1 == 540.0 {
                assert!(
                    visual
                        .debug_bounds("browser-picker-heading-library")
                        .unwrap()
                        .top()
                        < library.top(),
                    "the upper sources must actually scroll while collections stay fixed"
                );
            }
            editor.read_with(&visual, |view, _| {
                assert_eq!(view.state.as_ref().unwrap().project, before.project);
                assert_eq!(view.state.as_ref().unwrap().revision, before.revision);
                assert!(view.pending_requests.is_empty());
            });
        }
    }
}
