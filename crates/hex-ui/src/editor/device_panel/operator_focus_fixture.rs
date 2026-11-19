//! Render the real device panel in a private test window with a real Session.
use super::*;
use gpui::{AppContext, FocusHandle, Modifiers, TestAppContext, VisualTestContext};
use hex_project::Created;
use hex_session::service::{Event, Service};

fn receive_event(service: &Service) -> Event {
    let deadline = crate::test_wait::hang_deadline();
    loop {
        match service.try_event() {
            Ok(event) => {
                assert!(event.error.is_none(), "{:?}", event.error);
                return event;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            Err(error) => panic!("operator fixture response unavailable: {error}"),
        }
    }
}

fn operator_fixture() -> (Service, Arc<ViewState>, TrackId, DeviceId) {
    use hex_lattice_contract::DeviceTypeId;
    let service = Service::new(48_000, 8, 8_000_000).unwrap();
    receive_event(&service);
    let operator = hex_engine::default_native_device_state_for_class(
        NativeDeviceClass::Instrument,
        DeviceTypeId::from_static("hex.instrument.operator"),
    )
    .unwrap();
    service
        .submit(Request::Edit(Command::InsertTrackWithDevice {
            name: "device track".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: operator,
        }))
        .unwrap();
    let Created::Track(track) = receive_event(&service).created.unwrap() else {
        panic!("expected operator track");
    };
    service
        .submit(Request::AddNativeDevice {
            track,
            class: NativeDeviceClass::Effect,
            type_id: Box::new(DeviceTypeId::from_static("hex.utility")),
            before: None,
        })
        .unwrap();
    let state = receive_event(&service).state;
    let device = state
        .project
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    (service, state, track, device)
}

fn focused(visual: &mut VisualTestContext) -> FocusHandle {
    visual.update(|window, cx| window.focused(cx).expect("a tab target remains focused"))
}

/// Own every path reachable by this focus fixture, including its derived cache.
/// The unrelated cache entry models a different machine library without accessing any real machine library.
pub(super) struct OperatorTabBrowser {
    pub(super) root: std::path::PathBuf,
    expected_samples: Vec<std::path::PathBuf>,
}

impl OperatorTabBrowser {
    pub(super) fn new(saved_places: bool) -> Self {
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

    fn editor(
        &self,
        window: &mut Window,
        cx: &mut Context<EditorView>,
        service: Service,
        state: Arc<ViewState>,
        track: TrackId,
    ) -> EditorView {
        let mut editor =
            EditorView::new_for_test_with_browser_directory(window, cx, service, &self.root);
        // Inspect before running asynchronous work, after the construction path
        // has bound preferences, catalog paths and the first scan roots.
        assert_eq!(editor.browser.directory, self.root.join("samples"));
        assert_eq!(editor.browser.places, vec![self.root.join("samples")]);
        editor.state = Some(state);
        editor.selected_track = Some(track);
        editor.device_panel_active = true;
        editor.clip_view_open = true;
        editor.mode = crate::editor::EditorMode::Session;
        editor
    }

    #[cfg(test)]
    pub(super) fn window(
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
                window.replace_root(cx, |window, cx| {
                    self.editor(window, cx, service, state, track)
                })
            })
            .unwrap();
        (handle, editor)
    }

    pub(super) fn assert_bounded(&self, editor: &gpui::Entity<EditorView>, cx: &TestAppContext) {
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

/// Keep the real editor and Session behind the device controls without laying
/// out the browser, transport and Session grid again for each key press.
struct OperatorTabPanel(gpui::Entity<EditorView>);

impl gpui::Render for OperatorTabPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        self.0.update(cx, |editor, cx| {
            editor.wake_poll_if_busy();
            view(editor, Some(window), cx)
        })
    }
}

pub(crate) fn exercise(cx: &mut TestAppContext) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    cx.update(crate::editor::bind_editor_keys);
    let (service, state, track, device) = operator_fixture();
    let original = state.project.clone();
    let revision = state.revision;
    let browser = OperatorTabBrowser::new(false);
    let handle = cx.add_window(|window, cx| {
        let editor = cx.new(|cx| browser.editor(window, cx, service, state, track));
        cx.observe(&editor, |_, _, cx| cx.notify()).detach();
        OperatorTabPanel(editor)
    });
    let editor = handle.update(cx, |panel, _, _| panel.0.clone()).unwrap();
    let mut visual = VisualTestContext::from_window(*handle, cx);
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
