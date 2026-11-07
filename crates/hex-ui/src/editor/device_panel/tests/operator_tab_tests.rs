use super::*;
use gpui::{FocusHandle, Focusable};

fn focused(visual: &mut VisualTestContext) -> FocusHandle {
    visual.update(|window, cx| window.focused(cx).expect("a tab target remains focused"))
}

#[gpui::test]
fn operator_focus_paints_sections_without_mouse_motion_and_keeps_detail_dark(
    cx: &mut TestAppContext,
) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    cx.update(crate::editor::bind_editor_keys);
    let (service, state, track, device) = operator_navigation_tests::operator_fixture();
    let original = state.project.clone();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();
    let row = |title| {
        Box::leak(format!("operator-preview-row-{}-{title}", device.value()).into_boxed_str())
            as &'static str
    };
    let detail = Box::leak(format!("operator-detail-{}", device.value()).into_boxed_str());
    let background = |color| Some(gpui::Background::from(gpui::rgb(color)));
    let lfo = editor.read_with(cx, |view, app| {
        let ParameterControl::Bool { entity, .. } = &view.device_panel.controls[&ParameterKey {
            device,
            parameter: hex_lattice_contract::ParameterId::from_static(82),
        }] else {
            panic!("lfo toggle")
        };
        entity.focus_handle(app)
    });
    visual.update(|window, _| lfo.focus(window));
    visual.run_until_parked();
    assert_eq!(
        visual.rendered_background(row("lfo")),
        background(device_layout_style().surface)
    );
    for _ in 0..3 {
        visual.simulate_keystrokes("tab");
        assert_eq!(
            visual.rendered_background(row("lfo")),
            background(device_layout_style().surface)
        );
    }
    // Amount -> filter toggle, then its selector and two dials -> pitch toggle,
    // then three pitch dials -> global routing icon. No pointer event or forced draw.
    for (title, tabs) in [("filter", 1), ("pitch env", 4), ("global", 4)] {
        for _ in 0..tabs {
            visual.simulate_keystrokes("tab");
        }
        assert_eq!(
            visual.rendered_background(row(title)),
            background(device_layout_style().surface),
            "{title} paints immediately"
        );
        assert_eq!(
            visual.rendered_background(row("lfo")),
            background(device_layout_style().rail)
        );
    }
    visual.simulate_keystrokes("enter");
    let algorithm = editor.read_with(cx, |view, _| {
        view.device_panel.operator_navigation_focus
            [&(device, operator_panel::NavigationTarget::Algorithm(1))]
            .clone()
    });
    visual.update(|window, _| algorithm.focus(window));
    visual.run_until_parked();
    assert_eq!(
        visual.rendered_background(detail),
        background(device_layout_style().background),
        "focused detail stays dark"
    );
    let bounds = visual
        .debug_bounds(Box::leak(
            format!("operator-algorithm-{}-1", device.value()).into_boxed_str(),
        ))
        .unwrap();
    visual.simulate_click(bounds.center(), Modifiers::default());
    assert_eq!(
        visual.rendered_background(detail),
        background(device_layout_style().background),
        "pointer-focused detail stays dark"
    );
    editor.read_with(cx, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project, original)
    });
}

/// Own every path reachable by this focus fixture, including its derived cache.
/// The unrelated cache entry models a different machine library without accessing any real machine library.
struct OperatorTabBrowser {
    root: std::path::PathBuf,
    expected_samples: Vec<std::path::PathBuf>,
}

impl OperatorTabBrowser {
    fn new(saved_places: bool) -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "hex-operator-tab-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        std::fs::create_dir(&root).unwrap();
        let samples = root.join("samples");
        let unrelated = root.join("unrelated-library");
        std::fs::create_dir(&samples).unwrap();
        std::fs::create_dir(&unrelated).unwrap();
        let sample = samples.join("bounded.wav");
        let foreign = unrelated.join("not-an-operator-input.wav");
        // Tiny 44.1 kHz PCM takes the real waveform/resampling path, if focused.
        for path in [&sample, &foreign] {
            std::fs::write(path, Self::wav()).unwrap();
        }
        let mut expected_samples = vec![sample];
        let mut cached_roots = vec![samples, unrelated];
        if saved_places {
            let extra = root.join("saved-place");
            std::fs::create_dir(&extra).unwrap();
            let extra_sample = extra.join("saved.wav");
            std::fs::write(&extra_sample, Self::wav()).unwrap();
            hex_preferences::save(
                &root.join("browser.json"),
                &hex_preferences::BrowserPlaces::new([extra.clone()]).unwrap(),
            )
            .unwrap();
            expected_samples.push(extra_sample);
            cached_roots.push(extra);
        }
        // The normal cache bootstrap must filter this stale outside-root entry.
        hex_library::catalog::scan_samples(&cached_roots)
            .catalog
            .save(&root.join("browser.samples-v1.db"))
            .unwrap();
        Self {
            root,
            expected_samples,
        }
    }

    fn wav() -> Vec<u8> {
        let frames = 32_u32;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + frames * 2).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&44_100_u32.to_le_bytes());
        bytes.extend_from_slice(&88_200_u32.to_le_bytes());
        bytes.extend_from_slice(&2_u16.to_le_bytes());
        bytes.extend_from_slice(&16_u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&(frames * 2).to_le_bytes());
        bytes.extend_from_slice(&[0; 64]);
        bytes
    }

    fn window(
        &self,
        cx: &mut TestAppContext,
        service: Service,
        state: Arc<ViewState>,
        track: TrackId,
    ) -> (gpui::AnyWindowHandle, gpui::Entity<EditorView>) {
        let bootstrap = cx.add_window(|_, _| gpui::Empty);
        let handle = *bootstrap;
        let editor = cx
            .update_window(handle, |_, window, cx| {
                let editor = window.replace_root(cx, |window, cx| {
                    EditorView::new_for_test_with_browser_directory(window, cx, service, &self.root)
                });
                editor.update(cx, |view, cx| {
                    // Inspect before running asynchronous work; preferences/catalog paths
                    // and initial scan roots must be isolated at construction, not later.
                    assert_eq!(view.browser.directory, self.root.join("samples"));
                    assert_eq!(view.browser.places.clone(), vec![self.root.join("samples")]);
                    view.state = Some(state);
                    view.selected_track = Some(track);
                    view.device_panel_active = true;
                    view.clip_view_open = true;
                    view.mode = super::super::super::EditorMode::Session;
                    cx.notify();
                });
                editor
            })
            .unwrap();
        (handle, editor)
    }

    fn assert_bounded(&self, editor: &gpui::Entity<EditorView>, cx: &TestAppContext) {
        editor.read_with(cx, |view, _| {
            assert_eq!(view.browser.directory, self.root.join("samples"));
            let roots = view.browser.places.clone();
            assert!(roots.contains(&self.root.join("samples")));
            assert!(
                roots.iter().all(|root| root == &self.root.join("samples")
                    || root == &self.root.join("saved-place"))
            );
        });
        let catalog =
            hex_library::catalog::SampleCatalog::load(&self.root.join("browser.samples-v1.db"))
                .unwrap();
        let mut actual = catalog
            .entries()
            .iter()
            .map(|entry| entry.path.clone())
            .collect::<Vec<_>>();
        let mut expected = self.expected_samples.clone();
        actual.sort();
        expected.sort();
        assert_eq!(
            actual, expected,
            "only controlled media can enter browser preview"
        );
    }
}

impl Drop for OperatorTabBrowser {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[gpui::test]
fn operator_tab_cycles_rendered_controls_in_both_directions_without_project_edits(
    cx: &mut TestAppContext,
) {
    operator_tab_cycle(cx, false);
}

#[gpui::test]
fn operator_tab_cycle_with_saved_places_excludes_unrelated_cached_media(cx: &mut TestAppContext) {
    operator_tab_cycle(cx, true);
}

fn operator_tab_cycle(cx: &mut TestAppContext, saved_places: bool) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    cx.update(crate::editor::bind_editor_keys);
    let (service, state, track, device) = operator_navigation_tests::operator_fixture();
    let original = state.project.clone();
    let revision = state.revision;
    let browser = OperatorTabBrowser::new(saved_places);
    let (handle, editor) = browser.window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();
    browser.assert_bounded(&editor, cx);
    let coarse = visual
        .debug_bounds(Box::leak(
            format!("device-{}-parameter-10-slider", device.value()).into_boxed_str(),
        ))
        .unwrap();
    visual.simulate_click(coarse.center(), Modifiers::default());
    visual.run_until_parked();
    let first = focused(&mut visual);
    let mut order = vec![first.clone()];
    for _ in 0..96 {
        visual.simulate_keystrokes("tab");
        visual.run_until_parked();
        let next = focused(&mut visual);
        if order.contains(&next) {
            assert_eq!(
                next, first,
                "traversal wraps, rather than trapping in routing"
            );
            break;
        }
        order.push(next);
    }
    assert!(
        order.len() > 20,
        "oscillator, detail and section controls must be reachable"
    );
    assert_eq!(focused(&mut visual), first, "finite tab cycle");
    editor.read_with(cx, |view, _| {
        let page = view.device_panel.operator_pages[&device].page;
        for ((_, target), focus) in &view.device_panel.operator_navigation_focus {
            if matches!(target, operator_panel::NavigationTarget::Algorithm(_))
                && page != operator_panel::OperatorPage::Routing
                || matches!(target, operator_panel::NavigationTarget::Page(_))
                    && page == operator_panel::OperatorPage::Routing
            {
                continue; // Cached handles for a hidden page are outside the rendered tab order.
            }
            assert!(
                order.contains(focus),
                "oscillator and page selectors participate"
            );
        }
        let routing = view.device_panel.operator_previews[&device]
            .routing_focus
            .as_ref()
            .unwrap();
        assert!(order.contains(routing));
        assert_eq!(view.state.as_ref().unwrap().revision, revision);
        assert_eq!(view.state.as_ref().unwrap().project, original);
    });
    // The page follows shell/global focus as before. Set it to the final page
    // so backward traversal compares the same rendered control inventory.
    let mut reverse = Vec::new();
    for _ in 0..96 {
        visual.simulate_keystrokes("shift-tab");
        visual.run_until_parked();
        let previous = focused(&mut visual);
        if previous == first {
            break;
        }
        assert!(
            !reverse.contains(&previous),
            "backward traversal cannot trap"
        );
        reverse.push(previous);
    }
    assert!(reverse.len() > 20);
    assert_eq!(focused(&mut visual), first);
    editor.read_with(cx, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().revision, revision);
        assert_eq!(view.state.as_ref().unwrap().project, original);
    });
    browser.assert_bounded(&editor, cx);
}
#[gpui::test]
fn operator_section_activation_and_invalid_draft_keep_focus_and_state(cx: &mut TestAppContext) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    cx.update(crate::editor::bind_editor_keys);
    let (service, state, track, device) = operator_navigation_tests::operator_fixture();
    let revision = state.revision;
    let original = state.project.clone();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();
    for (page, key) in [
        (operator_panel::OperatorPage::Lfo, "enter"),
        (operator_panel::OperatorPage::Oscillator, "enter"),
    ] {
        let target = editor.read_with(cx, |view, _| {
            view.device_panel.operator_navigation_focus
                [&(device, operator_panel::NavigationTarget::Page(page))]
                .clone()
        });
        visual.update(|window, _| target.focus(window));
        visual.simulate_keystrokes(key);
        visual.run_until_parked();
        editor.read_with(cx, |view, _| {
            assert_eq!(view.device_panel.operator_pages[&device].page, page)
        });
        assert_eq!(focused(&mut visual), target);
    }
    // Badge keyboard activation is navigation only. Mouse enable toggles
    // must not leak into Enter or create a canonical history operation.
    for operator in 0..4 {
        let target = editor.read_with(cx, |view, _| {
            view.device_panel.operator_navigation_focus[&(
                device,
                operator_panel::NavigationTarget::Oscillator(operator),
            )]
                .clone()
        });
        visual.update(|window, _| target.focus(window));
        visual.simulate_keystrokes("enter");
        visual.run_until_parked();
        editor.read_with(cx, |view, _| {
            assert_eq!(
                view.device_panel.operator_pages[&device].selected_operator,
                operator
            );
            assert_eq!(view.state.as_ref().unwrap().revision, revision);
            assert_eq!(view.state.as_ref().unwrap().project, original);
            assert_eq!(view.worker_requests_pending, 0);
        });
        assert_eq!(focused(&mut visual), target);
    }
    let parameter = hex_lattice_contract::ParameterId::from_static(2);
    enter_typed_value(&mut visual, device, parameter, "invalid");
    let before = focused(&mut visual);
    visual.simulate_keystrokes("tab");
    visual.run_until_parked();
    assert_eq!(
        focused(&mut visual),
        before,
        "invalid draft refuses traversal"
    );
    editor.read_with(cx, |view, cx| {
        assert_eq!(view.state.as_ref().unwrap().revision, revision);
        let ParameterControl::Float { entity, .. } =
            &view.device_panel.controls[&ParameterKey { device, parameter }]
        else {
            panic!("numeric field")
        };
        assert!(entity.read(cx).is_editing());
        assert!(entity.focus_handle(cx) == before);
    });
    visual.simulate_keystrokes("escape");
    visual.simulate_keystrokes("tab");
    visual.run_until_parked();
    assert_ne!(focused(&mut visual), before);

    let original = editor.read_with(cx, |view, _| {
        view.state
            .as_ref()
            .unwrap()
            .project
            .track(track)
            .unwrap()
            .instrument()
            .unwrap()
            .state()
            .clone()
    });
    enter_typed_value(&mut visual, device, parameter, "-6 db");
    visual.simulate_keystrokes("tab");
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.pending_requests.is_empty()
            && view.device_gesture_barriers.is_empty()
            && view.device_gesture.snapshot().is_none()
    });
    visual.run_until_parked();
    let authored = editor.read_with(cx, |view, _| {
        view.state
            .as_ref()
            .unwrap()
            .project
            .track(track)
            .unwrap()
            .instrument()
            .unwrap()
            .state()
            .clone()
    });
    assert_ne!(authored, original, "valid typing commits before traversal");
    visual.simulate_keystrokes("cmd-z");
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0 && view.pending_requests.is_empty()
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .track(track)
                .unwrap()
                .instrument()
                .unwrap()
                .state(),
            &original,
            "one Undo restores the full canonical state"
        )
    });
    visual.simulate_keystrokes("cmd-shift-z");
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0 && view.pending_requests.is_empty()
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .track(track)
                .unwrap()
                .instrument()
                .unwrap()
                .state(),
            &authored
        )
    });
}

#[gpui::test]
fn operator_tab_targets_retire_on_track_owner_change(cx: &mut TestAppContext) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    cx.update(crate::editor::bind_editor_keys);
    let (service, _, track, device) = operator_navigation_tests::operator_fixture();
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "other".into(),
        }))
        .unwrap();
    let event = receive_service_event(&service);
    let Created::Track(other) = event.created.unwrap() else {
        panic!("track")
    };
    let revision = event.state.revision;
    let original = event.state.project.clone();
    let (handle, editor) = actor_editor_window(cx, service, event.state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();
    let key = (
        device,
        operator_panel::NavigationTarget::Page(operator_panel::OperatorPage::Lfo),
    );
    let old = editor.read_with(cx, |view, _| {
        view.device_panel.operator_navigation_focus[&key].clone()
    });
    editor.update(cx, |view, cx| {
        view.selected_track = Some(other);
        cx.notify();
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(view.device_panel.operator_navigation_focus.is_empty())
    });
    visual.update(|window, _| old.focus(window));
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().revision, revision);
        assert_eq!(view.state.as_ref().unwrap().project, original);
        assert!(view.device_panel.operator_pages.is_empty());
    });
    editor.update(cx, |view, cx| {
        view.selected_track = Some(track);
        cx.notify();
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_ne!(view.device_panel.operator_navigation_focus[&key], old)
    });
}

#[gpui::test]
fn operator_shell_controls_precede_selectors_and_sections_follow_child_focus(
    cx: &mut TestAppContext,
) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    cx.update(crate::editor::bind_editor_keys);
    let (service, state, track, device) = operator_navigation_tests::operator_fixture();
    let original = state.project.clone();
    let revision = state.revision;
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();
    for operator in 0..4 {
        let bounds = |visual: &mut VisualTestContext, selector: String| {
            visual
                .debug_bounds(Box::leak(selector.into_boxed_str()))
                .unwrap()
        };
        let selector = bounds(
            &mut visual,
            format!("operator-select-{}-{operator}", device.value()),
        );
        let level = bounds(
            &mut visual,
            format!(
                "device-{}-parameter-{}-slider",
                device.value(),
                operator * 3 + 2
            ),
        );
        let row = bounds(
            &mut visual,
            format!("operator-row-{}-{operator}", device.value()),
        );
        assert!(
            selector.origin.x >= level.origin.x + level.size.width,
            "oscillator squares stay at the far right"
        );
        assert!(selector.origin.x + selector.size.width <= row.origin.x + row.size.width);
    }
    let order = editor.read_with(cx, |view, cx| {
        let mut order = Vec::new();
        for operator in (0..4).rev() {
            let base = operator as u32 * 3 + 1;
            let focus = |parameter| match &view.device_panel.controls[&ParameterKey {
                device,
                parameter: hex_lattice_contract::ParameterId::from_static(parameter),
            }] {
                ParameterControl::Float { entity, .. } => entity.focus_handle(cx),
                ParameterControl::Bool { entity, .. } => entity.focus_handle(cx),
                ParameterControl::Choice { entity, .. } => entity.focus_handle(cx),
            };
            order.push(focus(base));
            order.push(focus(609 + operator as u32));
            order.push(focus(62 + operator as u32 * 3));
            order.push(focus(base + 1));
            order.push(
                view.device_panel.operator_navigation_focus[&(
                    device,
                    operator_panel::NavigationTarget::Oscillator(operator),
                )]
                    .clone(),
            );
        }
        order
    });
    visual.update(|window, _| order[0].focus(window));
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.device_panel.operator_pages[&device].selected_operator, 0,
            "working-control focus preserves the explicit oscillator selection"
        )
    });
    for (index, expected) in order.iter().enumerate().skip(1) {
        visual.simulate_keystrokes("tab");
        visual.run_until_parked();
        assert_eq!(
            &focused(&mut visual),
            expected,
            "coarse, fine, fixed, level, selector"
        );
        for operator in 0..4 {
            let row =
                Box::leak(format!("operator-row-{}-{operator}", device.value()).into_boxed_str());
            let color = if operator == 3 - index / 5 {
                device_layout_style().surface
            } else {
                device_layout_style().rail
            };
            assert_eq!(
                visual.rendered_background(row),
                Some(gpui::Background::from(gpui::rgb(color))),
                "row paints the current keyboard target"
            );
        }
    }
    for expected in order.iter().rev().skip(1) {
        visual.simulate_keystrokes("shift-tab");
        visual.run_until_parked();
        assert_eq!(&focused(&mut visual), expected);
    }
    // Preview child focus reveals its row without making the row a Tab stop
    // or changing its switch, project state, or neighboring row's focus mode.
    for (field, title) in [
        (operator_panel::preview::Field::PitchEnvelope, "pitch env"),
        (operator_panel::preview::Field::Time, "global"),
    ] {
        let child = editor.read_with(cx, |view, cx| {
            view.device_panel.operator_previews[&device].controls[&field]
                .entity
                .focus_handle(cx)
        });
        visual.update(|window, _| child.focus(window));
        visual.run_until_parked();
        editor.read_with(cx, |view, _| {
            for ((_, section), state) in &view.device_panel.operator_section_focus {
                assert!(
                    !state.handle.tab_stop,
                    "containers are excluded from traversal"
                );
                assert_eq!(
                    state.focused,
                    *section == operator_panel::Section::Preview(title)
                );
            }
            assert_eq!(
                view.device_panel.operator_previews[&device].enabled,
                [true; 1]
            );
            assert_eq!(view.state.as_ref().unwrap().project, original);
            assert_eq!(view.state.as_ref().unwrap().revision, revision);
        });
        assert_eq!(focused(&mut visual), child);
    }
}

#[gpui::test]
fn operator_preview_invalid_drafts_refuse_forward_and_reverse_tab(cx: &mut TestAppContext) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    cx.update(crate::editor::bind_editor_keys);
    let (service, state, track, device) = operator_navigation_tests::operator_fixture();
    let original = state.project.clone();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();
    for (field, valid) in [
        (operator_panel::preview::Field::PitchEnvelope, "20"),
        (operator_panel::preview::Field::Time, "30"),
    ] {
        let selector = Box::leak(
            format!(
                "operator-preview-{}-{}-value",
                device.value(),
                field as usize
            )
            .into_boxed_str(),
        );
        let bounds = visual.debug_bounds(selector).unwrap();
        visual.simulate_click(bounds.center(), Modifiers::default());
        visual.simulate_keystrokes("cmd-a");
        visual.simulate_input("invalid");
        visual.run_until_parked();
        let before = focused(&mut visual);
        for key in ["tab", "shift-tab"] {
            visual.simulate_keystrokes(key);
            visual.run_until_parked();
            assert_eq!(
                focused(&mut visual),
                before,
                "invalid preview draft must keep focus"
            );
            editor.read_with(cx, |view, cx| {
                assert!(
                    view.device_panel.operator_previews[&device].controls[&field]
                        .entity
                        .read(cx)
                        .is_editing()
                )
            });
        }
        visual.simulate_keystrokes("cmd-a");
        visual.simulate_input(valid);
        visual.simulate_keystrokes("tab");
        visual.run_until_parked();
        assert_ne!(
            focused(&mut visual),
            before,
            "corrected draft permits traversal"
        );
        editor.read_with(cx, |view, cx| {
            assert!(
                !view.device_panel.operator_previews[&device].controls[&field]
                    .entity
                    .read(cx)
                    .is_editing()
            );
            assert_eq!(
                view.device_panel.operator_previews[&device].values[field as usize],
                valid.parse::<f64>().unwrap()
            );
            assert_eq!(
                view.state.as_ref().unwrap().project,
                original,
                "view-only previews do not author sound"
            );
        });
    }
}

#[gpui::test]
fn operator_lfo_and_routing_widgets_have_one_focus_group_and_release_previous_group(
    cx: &mut TestAppContext,
) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    cx.update(crate::editor::bind_editor_keys);
    let (service, state, track, device) = operator_navigation_tests::operator_fixture();
    let original = state.project.clone();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();
    let parameter_focus = |parameter, cx: &TestAppContext| {
        editor.read_with(cx, |view, app| {
            match &view.device_panel.controls[&ParameterKey {
                device,
                parameter: hex_lattice_contract::ParameterId::from_static(parameter),
            }] {
                ParameterControl::Float { entity, .. } => entity.focus_handle(app),
                ParameterControl::Bool { entity, .. } => entity.focus_handle(app),
                ParameterControl::Choice { entity, .. } => entity.focus_handle(app),
            }
        })
    };
    let lfo_background = visual
        .debug_bounds(Box::leak(
            format!("operator-lfo-link-{}", device.value()).into_boxed_str(),
        ))
        .unwrap();
    visual.simulate_click(lfo_background.center(), Modifiers::default());
    visual.run_until_parked();
    let lfo = parameter_focus(82, cx);
    visual.update(|window, _| lfo.focus(window));
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.device_panel.operator_pages[&device].page,
            operator_panel::OperatorPage::Lfo
        )
    });
    // A direct focus update changes page context; draw its newly mounted controls
    // before looking up their handles (native windows draw on the next frame).
    visual.update(|window, cx| {
        window.draw(cx).clear();
    });
    visual.run_until_parked();
    let sync = editor.read_with(cx, |view, cx| {
        let ParameterControl::Bool { entity, .. } = &view.device_panel.controls[&ParameterKey {
            device,
            parameter: hex_lattice_contract::ParameterId::from_static(84),
        }] else {
            panic!("sync toggle");
        };
        entity.read(cx).checked()
    });
    let alternate_rate = if sync { 85 } else { 86 };
    let detail_order: Vec<_> = [84, alternate_rate, 88, 89, 90, 91, 92]
        .into_iter()
        .map(|parameter| parameter_focus(parameter, cx))
        .collect();
    visual.update(|window, _| detail_order[0].focus(window));
    visual.run_until_parked();
    for target in detail_order.iter().skip(1) {
        visual.simulate_keystrokes("tab");
        visual.run_until_parked();
        assert_eq!(
            &focused(&mut visual),
            target,
            "LFO detail has no duplicate summary widgets"
        );
    }
    for target in detail_order.iter().rev().skip(1) {
        visual.simulate_keystrokes("shift-tab");
        visual.run_until_parked();
        assert_eq!(&focused(&mut visual), target);
    }
    for parameter in 82..=92 {
        let target = parameter_focus(parameter, cx);
        visual.update(|window, _| target.focus(window));
        visual.run_until_parked();
        let section =
            if matches!(parameter, 82 | 83 | 87) || parameter == if sync { 86 } else { 85 } {
                operator_panel::Section::Preview("lfo")
            } else {
                operator_panel::Section::Detail
            };
        visual.update(|window, app| {
            editor.read_with(app, |view, _| {
                for ((_, candidate), state) in &view.device_panel.operator_section_focus {
                    assert_eq!(
                        state.focused,
                        *candidate == section,
                        "one active group for LFO parameter {parameter}"
                    );
                    assert_eq!(
                        state.handle.contains_focused(window, app),
                        *candidate == section,
                        "rendered ancestry matches focus mode"
                    );
                }
            })
        });
    }
    // The routing icon is a widget target, followed by detail's FM algorithm selector.
    let routing = editor.read_with(cx, |view, _| {
        view.device_panel.operator_previews[&device]
            .routing_focus
            .as_ref()
            .unwrap()
            .clone()
    });
    visual.update(|window, _| routing.focus(window));
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    let algorithm = editor.read_with(cx, |view, _| {
        view.device_panel.operator_navigation_focus
            [&(device, operator_panel::NavigationTarget::Algorithm(1))]
            .clone()
    });
    visual.update(|window, _| algorithm.focus(window));
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        for ((_, section), state) in &view.device_panel.operator_section_focus {
            assert_eq!(
                state.focused,
                *section == operator_panel::Section::Detail,
                "algorithm focus releases the global and LFO rows"
            );
        }
        assert_eq!(view.state.as_ref().unwrap().project, original);
    });
    let knob = visual
        .debug_bounds(Box::leak(
            format!("device-{}-parameter-87-knob", device.value()).into_boxed_str(),
        ))
        .unwrap();
    visual.simulate_mouse_down(knob.center(), MouseButton::Left, Modifiers::default());
    let moved = point(knob.center().x, knob.center().y - px(20.0));
    visual.simulate_mouse_move(moved, Some(MouseButton::Left), Modifiers::default());
    visual.simulate_mouse_up(moved, MouseButton::Left, Modifiers::default());
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.pending_requests.is_empty()
            && view.device_gesture.snapshot().is_none()
    });
    editor.read_with(cx, |view, _| {
        assert_ne!(
            view.state.as_ref().unwrap().project,
            original,
            "LFO focus activation preserves the initiating pointer gesture"
        );
    });
    visual.simulate_keystrokes("cmd-z");
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0 && view.pending_requests.is_empty()
    });
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.state.as_ref().unwrap().project,
            original,
            "one Undo restores the LFO drag"
        )
    });
    let outside = editor.read_with(cx, |view, _| view.focus.clone());
    visual.update(|window, _| outside.focus(window));
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(
            view.device_panel
                .operator_section_focus
                .values()
                .all(|state| !state.focused),
            "leaving Operator deactivates every section"
        )
    });
}

#[gpui::test]
fn operator_filter_shared_controls_focus_commit_undo_and_redo(cx: &mut TestAppContext) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    cx.update(crate::editor::bind_editor_keys);
    let (service, state, track, device) = operator_navigation_tests::operator_fixture();
    let original = state.project.clone();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1440.0), px(540.0)));
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();
    let left = visual
        .debug_bounds(Box::leak(
            format!("device-{}-parameter-20001", device.value()).into_boxed_str(),
        ))
        .unwrap();
    let right = visual
        .debug_bounds(Box::leak(
            format!("device-{}-parameter-20002", device.value()).into_boxed_str(),
        ))
        .unwrap();
    visual.simulate_click(
        point((left.right() + right.left()) / 2.0, left.center().y),
        Modifiers::default(),
    );
    visual.run_until_parked();
    let focus = editor.read_with(cx, |view, app| {
        let ParameterControl::Float { entity, .. } = &view.device_panel.controls[&ParameterKey {
            device,
            parameter: hex_lattice_contract::ParameterId::from_static(20005),
        }] else {
            panic!("frequency")
        };
        entity.focus_handle(app)
    });
    visual.update(|window, _| focus.focus(window));
    visual.run_until_parked();
    visual.update(|window, cx| {
        window.draw(cx).clear();
    });
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.device_panel.operator_pages[&device].page,
            operator_panel::OperatorPage::Filter
        );
        assert_eq!(view.state.as_ref().unwrap().project, original);
        for parameter in 20001..=20007 {
            assert!(view.device_panel.controls.contains_key(&ParameterKey {
                device,
                parameter: hex_lattice_contract::ParameterId::from_static(parameter)
            }));
        }
    });
    let knob = visual
        .debug_bounds(Box::leak(
            format!("device-{}-parameter-20005-knob", device.value()).into_boxed_str(),
        ))
        .unwrap();
    let start = knob.center();
    let moved = point(start.x, start.y - px(20.0));
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(moved, Some(MouseButton::Left), Modifiers::default());
    visual.simulate_mouse_up(moved, MouseButton::Left, Modifiers::default());
    pump_until(&editor, cx, |v| {
        v.worker_requests_pending == 0
            && v.pending_requests.is_empty()
            && v.device_gesture.snapshot().is_none()
    });
    let changed = editor.read_with(cx, |v, _| v.state.as_ref().unwrap().project.clone());
    assert_ne!(changed, original);
    visual.simulate_keystrokes("cmd-z");
    pump_until(&editor, cx, |v| {
        v.worker_requests_pending == 0 && v.pending_requests.is_empty()
    });
    editor.read_with(cx, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project, original)
    });
    visual.simulate_keystrokes("cmd-shift-z");
    pump_until(&editor, cx, |v| {
        v.worker_requests_pending == 0 && v.pending_requests.is_empty()
    });
    editor.read_with(cx, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project, changed)
    });
    // Commit a typed value and immediately Undo through the normal shortcut
    // after moving focus. This exercises the pending terminal/barrier path.
    let field = visual
        .debug_bounds(Box::leak(
            format!("device-{}-parameter-20005-value", device.value()).into_boxed_str(),
        ))
        .unwrap();
    visual.simulate_click(field.center(), Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("1375");
    visual.simulate_keystrokes("enter tab cmd-z");
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.pending_requests.is_empty()
            && view.device_gesture_barriers.is_empty()
            && view.device_gesture.snapshot().is_none()
    });
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.state.as_ref().unwrap().project,
            changed,
            "one keyboard Undo restores the entire pre-typed state"
        );
    });
    visual.simulate_keystrokes("cmd-shift-z");
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.pending_requests.is_empty()
            && view.device_gesture_barriers.is_empty()
            && view.device_gesture.snapshot().is_none()
    });
    let changed = editor.read_with(cx, |view, _| {
        let project = view.state.as_ref().unwrap().project.clone();
        assert_eq!(
            hex_engine::native_device_parameter_for_class(
                NativeDeviceClass::Instrument,
                project.device(device).unwrap().state(),
                hex_lattice_contract::ParameterId::from_static(20005),
            )
            .unwrap(),
            ParameterValue::Float(1375.0)
        );
        project
    });
    let path = std::env::temp_dir().join(format!(
        "hex-filter-{}-{}.hex",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    editor.update(cx, |v, _| {
        assert!(v.enqueue(Request::SaveNew(path.clone())))
    });
    pump_until(&editor, cx, |v| {
        v.worker_requests_pending == 0 && v.pending_requests.is_empty()
    });
    let reload = Service::new(48000, 8, 8_000_000).unwrap();
    let _ = receive_service_event(&reload);
    reload.submit(Request::Open(path.clone())).unwrap();
    let reopened = receive_service_event(&reload);
    assert!(reopened.error.is_none());
    assert_eq!(
        reopened.state.project.device(device).unwrap().state(),
        changed.device(device).unwrap().state()
    );
    std::fs::remove_file(path).unwrap();
}
