//! Rendered browser plus original native worker/endpoint/PCM. Catalog transport
//! metadata is a bounded fixture; its source digests come from real inspection.
//! No fixture grants admission, native facts, endpoint ACK or canonical edits.
use super::*;
use gpui::{Modifiers, MouseButton, TestAppContext, VisualTestContext};
use std::path::Path;

pub(super) fn repaint(cx: &mut TestAppContext, visual: &mut VisualTestContext) {
    cx.background_executor
        .advance_clock(Duration::from_millis(16));
    visual.run_until_parked();
}

pub(super) fn click(visual: &mut VisualTestContext, selector: &'static str) {
    let bounds = visual
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("missing painted {selector}"));
    // The Main header also contains mixer controls. Select its name area;
    // clicking the whole header's centre can start a real numeric gesture.
    let position = if selector == "main-track-header" {
        gpui::point(bounds.origin.x + gpui::px(8.0), bounds.center().y)
    } else {
        bounds.center()
    };
    visual.simulate_click(position, Modifiers::default());
    visual.run_until_parked();
}

pub(super) fn until(label: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ready() {
        assert!(Instant::now() < deadline, "{label}");
        std::thread::yield_now();
    }
}

struct BrowserFixture {
    window: gpui::AnyWindowHandle,
    visual: VisualTestContext,
    state: Arc<hex_session::service::ViewState>,
    expected_owner: PluginEffectDestination,
    track: hex_project::TrackId,
    callback: Arc<Mutex<Option<HeadlessBackend>>>,
    queued: Arc<AtomicBool>,
    completed: Arc<AtomicU64>,
    records: Arc<std::sync::atomic::AtomicUsize>,
    progress: Arc<Mutex<crate::native_audio::plugins::PreparationObservation>>,
    facts: mpsc::Receiver<()>,
    resume: mpsc::SyncSender<()>,
    original_catalog: Vec<u8>,
    module: PathBuf,
    files: Files,
}

fn open_browser(
    cx: &mut TestAppContext,
    location: u8,
    presentation: Option<EffectPresentation>,
) -> BrowserFixture {
    open_browser_metadata(cx, location, presentation, "Fx", 0, false)
}

pub(super) fn copy_original_module(source: &Path, target: &Path) {
    fs::create_dir(target).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let destination = target.join(entry.file_name());
        let kind = entry.file_type().unwrap();
        assert!(!kind.is_symlink(), "original fixture must contain no links");
        if kind.is_dir() {
            copy_original_module(&entry.path(), &destination);
        } else {
            assert!(kind.is_file());
            fs::copy(entry.path(), destination).unwrap();
        }
    }
}

fn open_browser_metadata(
    cx: &mut TestAppContext,
    location: u8,
    presentation: Option<EffectPresentation>,
    subcategories: &str,
    factory_flags: u32,
    seed_anchor: bool,
) -> BrowserFixture {
    let (reached, facts) = mpsc::sync_channel(1);
    let (resume, resumed) = mpsc::sync_channel(1);
    let mut h = ResidentHarness::with_plugins_paused_at_facts(
        host(),
        presentation,
        crate::native_audio::plugins::FactsGate::new(reached, resumed),
    );
    assert!(h.next().error.is_none());
    let (mut applied, _) = h.applied(0);
    let files = Files::new();
    let mut state = h.run(Request::Import(files.wav(48_000, 4096))).state;
    (applied, _) = h.applied(applied);
    let track = state.project.tracks()[0].id();
    let mut target = format!("track-name-{}", track.value());
    if location == 1 {
        let added = h.run(Request::Edit(Command::AddReturnTrack {
            name: "effect".into(),
        }));
        (applied, _) = h.applied(applied);
        let Some(Created::ReturnTrack(bus)) = added.created else {
            panic!("return");
        };
        state = h
            .run(Request::Edit(Command::SetReturnSend {
                track,
                return_track: bus,
                gain: 0.5,
            }))
            .state;
        (applied, _) = h.applied(applied);
        target = format!("return-track-focus-{}", bus.value());
    } else if location == 2 {
        target = "main-track-header".into();
    }
    let expected_owner = match location {
        0 => PluginEffectDestination::Track(track),
        1 => PluginEffectDestination::Return(state.project.return_tracks()[0].id()),
        _ => PluginEffectDestination::Main,
    };
    if seed_anchor {
        let utility = hex_engine::default_native_device_state_for_class(
            hex_engine::NativeDeviceClass::Effect,
            hex_engine::DeviceTypeId::from_static("hex.utility"),
        )
        .unwrap();
        let command = match expected_owner {
            PluginEffectDestination::Track(track) => Command::InsertDevice {
                track,
                before: None,
                state: utility,
            },
            PluginEffectDestination::Return(return_track) => Command::InsertReturnDevice {
                return_track,
                before: None,
                state: utility,
            },
            PluginEffectDestination::Instrument(_) => panic!("effect fixture destination"),
            PluginEffectDestination::Main => Command::InsertMainDevice {
                before: None,
                state: utility,
            },
        };
        state = h.run(Request::Edit(command)).state;
        h.applied(applied);
    }
    let mut selected = selection();
    // Each rendered test owns its bundle copy. Missing/replacement controls
    // must never modify the shared qualified original fixture or user modules.
    let module = files.0.canonicalize().unwrap().join("original.vst3");
    copy_original_module(&selected.module, &module);
    selected.module = module.clone();
    hex_ui::write_plugin_effect_test_catalog(&files.0, &selected, subcategories, factory_flags)
        .unwrap();
    let original_catalog = fs::read(files.0.join("plugin-catalog.json")).unwrap();
    // A real read-only request supplies the latest canonical snapshot after
    // fixture authoring; the view never receives a fabricated project event.
    let clip = state.project.track(track).unwrap().clips()[0].id();
    h.service.submit(Request::CopyClips(vec![clip])).unwrap();
    let callback = Arc::clone(&h.callback);
    let queued = Arc::clone(&h.plugin_queued);
    let completed = Arc::clone(&h.plugin_completed);
    let records = Arc::clone(&h.plugin_records);
    let progress = Arc::clone(&h.plugin_progress);
    let backend = h.into_plugin_ui_backend();
    let window =
        hex_ui::open_sampler_load_test_backend(cx, backend, files.0.clone(), files.0.clone())
            .unwrap();
    let mut visual = VisualTestContext::from_window(window, cx);
    visual.update(|window, _| window.activate_window());
    until("initial canonical snapshot", || {
        repaint(cx, &mut visual);
        cx.update(|app| hex_ui::plugin_effect_test_view(window, app))
            .unwrap()
            .state
            .as_ref()
            .is_some_and(|s| s.revision == state.revision)
    });
    click(&mut visual, Box::leak(target.into_boxed_str()));
    assert_eq!(
        cx.update(|app| hex_ui::plugin_effect_test_view(window, app))
            .unwrap()
            .selected_owner,
        Some(expected_owner)
    );
    click(&mut visual, "browser-section-vst3");
    until("persisted catalog row paint", || {
        repaint(cx, &mut visual);
        visual.debug_bounds("browser-vst3-row-0").is_some()
    });
    BrowserFixture {
        window,
        visual,
        state,
        expected_owner,
        track,
        callback,
        queued,
        completed,
        records,
        progress,
        facts,
        resume,
        original_catalog,
        module,
        files,
    }
}

impl BrowserFixture {
    fn view(&self, cx: &mut TestAppContext) -> hex_ui::PluginEffectTestView {
        cx.update(|app| hex_ui::plugin_effect_test_view(self.window, app))
            .unwrap()
    }
    fn step(&mut self, cx: &mut TestAppContext) {
        let mut slot = self.callback.lock().unwrap();
        if slot
            .as_ref()
            .is_some_and(HeadlessBackend::shutdown_requested)
        {
            slot.take().unwrap().close();
        } else if let Some(backend) = slot.as_mut() {
            backend
                .render_native_direct(&mut [[0.0; 2]; 64], &mut [0.0; 128])
                .unwrap();
        }
        drop(slot);
        repaint(cx, &mut self.visual);
    }
    fn activate_to_facts(&mut self, cx: &mut TestAppContext, pointer: bool) {
        self.activate(pointer);
        until("actual native facts before endpoint construction", || {
            repaint(cx, &mut self.visual);
            let view = self.view(cx);
            assert!(view.error.is_none() && view.pending, "{:?}", view.error);
            self.facts.try_recv().is_ok()
        });
    }
    fn activate(&mut self, pointer: bool) {
        let row = self.visual.debug_bounds("browser-vst3-row-0").unwrap();
        if pointer {
            self.visual.simulate_event(gpui::MouseDownEvent {
                position: row.center(),
                button: MouseButton::Left,
                click_count: 2,
                first_mouse: false,
                modifiers: Modifiers::default(),
            });
            self.visual
                .simulate_mouse_up(row.center(), MouseButton::Left, Modifiers::default());
        } else {
            self.visual
                .simulate_click(row.center(), Modifiers::default());
            self.visual.simulate_keystrokes("enter");
        }
    }
}

#[gpui::test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn rendered_vst3_effect_drop_inserts_before_the_pointed_device_on_each_owner(
    cx: &mut TestAppContext,
) {
    let _native = native_fixture();
    for location in 0..3 {
        let mut fixture = open_browser_metadata(
            cx,
            location,
            Some(EffectPresentation {
                input_frames: 0,
                output_frames: 512,
            }),
            "Fx",
            0,
            true,
        );
        let owner = fixture.expected_owner;
        let devices = |state: &hex_session::service::ViewState| -> Vec<hex_project::DeviceId> {
            match owner {
                PluginEffectDestination::Track(id) => state.project.track(id).unwrap().devices(),
                PluginEffectDestination::Return(id) => state
                    .project
                    .return_tracks()
                    .iter()
                    .find(|bus| bus.id() == id)
                    .unwrap()
                    .devices(),
                PluginEffectDestination::Instrument(_) => panic!("effect fixture destination"),
                PluginEffectDestination::Main => state.project.main_devices(),
            }
            .iter()
            .map(|device| device.id())
            .collect()
        };
        let anchor = devices(&fixture.state)[0];
        let selector = match owner {
            PluginEffectDestination::Track(_) => format!("device-drop-card-{}", anchor.value()),
            PluginEffectDestination::Return(_) => {
                format!("return-device-drop-card-{}", anchor.value())
            }
            PluginEffectDestination::Instrument(_) => panic!("effect fixture destination"),
            PluginEffectDestination::Main => format!("main-device-drop-card-{}", anchor.value()),
        };
        let row = fixture.visual.debug_bounds("browser-vst3-row-0").unwrap();
        let start = row.center();
        let modifiers = Modifiers::default();
        let begin_drag = |visual: &mut VisualTestContext| {
            visual.simulate_mouse_down(start, MouseButton::Left, modifiers);
            visual.simulate_mouse_move(
                gpui::point(start.x + gpui::px(12.0), start.y),
                MouseButton::Left,
                modifiers,
            );
            visual.run_until_parked();
        };
        begin_drag(&mut fixture.visual);
        fixture.visual.simulate_keystrokes("escape");
        fixture
            .visual
            .simulate_mouse_up(start, MouseButton::Left, modifiers);
        repaint(cx, &mut fixture.visual);
        assert!(!fixture.view(cx).pending);
        assert_eq!(
            fixture.view(cx).state.unwrap().revision,
            fixture.state.revision
        );
        assert_eq!(fixture.records.load(Acquire), 0);

        begin_drag(&mut fixture.visual);
        let card = fixture
            .visual
            .debug_bounds(Box::leak(selector.into_boxed_str()))
            .expect("painted device-chain insertion target");
        let before = gpui::point(card.origin.x + gpui::px(10.0), card.center().y);
        fixture
            .visual
            .simulate_mouse_move(before, MouseButton::Left, modifiers);
        repaint(cx, &mut fixture.visual);
        assert!(
            !fixture.view(cx).pending,
            "hover only must not admit a plug-in"
        );
        fixture
            .visual
            .simulate_mouse_up(before, MouseButton::Left, modifiers);
        until("drop reaches original native worker", || {
            repaint(cx, &mut fixture.visual);
            let view = fixture.view(cx);
            assert!(
                view.pending && view.error.is_none(),
                "location {location}: {:?}",
                view.error
            );
            fixture.facts.try_recv().is_ok()
        });
        fixture.resume.send(()).unwrap();
        until("accepted drop and native endpoint", || {
            fixture.step(cx);
            let view = fixture.view(cx);
            assert!(
                view.error.is_none(),
                "location {location}: {:?}",
                view.error
            );
            !view.pending
                && view.applied.is_some_and(|source| {
                    source.context.editor_revision == fixture.state.revision + 1
                })
        });
        let accepted = fixture.view(cx).state.unwrap();
        let ids = devices(&accepted);
        assert_eq!(ids.len(), 2);
        assert_eq!(ids[0].value(), fixture.state.project.next_identity());
        assert_eq!(ids[1], anchor);
        assert_eq!(fixture.records.load(Acquire), 1);
        fixture.visual.update(|window, _| window.remove_window());
        fixture.visual.run_until_parked();
    }
}

#[derive(Clone, Copy, Debug)]
enum InvalidSource {
    Missing,
    ReplacedExecutable,
    ReplacedInfo,
    InstrumentOnly,
    UnknownType,
    DynamicInventory,
}

#[gpui::test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn rendered_native_effect_refuses_changed_sources_and_unsupported_inventory_before_spawn(
    cx: &mut TestAppContext,
) {
    let _native = native_fixture();
    for (invalid, pointer) in [
        InvalidSource::Missing,
        InvalidSource::ReplacedExecutable,
        InvalidSource::ReplacedInfo,
        InvalidSource::InstrumentOnly,
        InvalidSource::UnknownType,
        InvalidSource::DynamicInventory,
    ]
    .into_iter()
    .flat_map(|invalid| [false, true].map(move |pointer| (invalid, pointer)))
    {
        let categories = match invalid {
            InvalidSource::InstrumentOnly => "Instrument|Synth",
            InvalidSource::UnknownType => "MyFx",
            _ => "Fx",
        };
        let flags = u32::from(matches!(invalid, InvalidSource::DynamicInventory));
        let mut fixture = open_browser_metadata(
            cx,
            2,
            Some(EffectPresentation {
                input_frames: 0,
                output_frames: 512,
            }),
            categories,
            flags,
            false,
        );
        // Change the owned source only after the last-good catalog has loaded
        // and its actual row has painted. Admission must revalidate at use.
        match invalid {
            InvalidSource::Missing => fs::remove_dir_all(&fixture.module).unwrap(),
            InvalidSource::ReplacedExecutable => {
                let path = fixture.module.join("Contents/MacOS/fixture");
                let mut bytes = fs::read(&path).unwrap();
                bytes.push(0);
                fs::write(path, bytes).unwrap();
            }
            InvalidSource::ReplacedInfo => {
                let path = fixture.module.join("Contents/Info.plist");
                let mut bytes = fs::read(&path).unwrap();
                bytes.push(b'\n');
                fs::write(path, bytes).unwrap();
            }
            _ => {}
        }
        fixture.activate(pointer);
        until("actual source/metadata refusal terminal", || {
            fixture.step(cx);
            let view = fixture.view(cx);
            assert_eq!(
                fixture.records.load(Acquire),
                0,
                "{invalid:?} pointer={pointer}: unexpected native source reservation"
            );
            !view.pending && view.error.is_some()
        });
        let expected = match invalid {
            InvalidSource::Missing => "instance source refused",
            InvalidSource::ReplacedExecutable | InvalidSource::ReplacedInfo => {
                "selected vst3 source changed"
            }
            InvalidSource::InstrumentOnly => "select a track for the vst3 instrument",
            InvalidSource::UnknownType => "does not report the requested role",
            InvalidSource::DynamicInventory => "requires a rescan",
        };
        assert!(
            fixture.view(cx).error.unwrap().contains(expected),
            "{invalid:?} pointer={pointer}: {:?}",
            fixture.view(cx).error
        );
        for _ in 0..3 {
            fixture.step(cx);
            let view = fixture.view(cx);
            let state = view.state.as_ref().unwrap();
            assert!(!view.pending && view.applied.is_none());
            assert_eq!(state.project, fixture.state.project, "{invalid:?}");
            assert_eq!(
                (state.revision, state.can_undo, state.can_redo),
                (
                    fixture.state.revision,
                    fixture.state.can_undo,
                    fixture.state.can_redo
                )
            );
            assert_eq!(fixture.records.load(Acquire), 0);
            assert_eq!(fixture.completed.load(Acquire), 0);
            assert!(fixture.facts.try_recv().is_err());
            assert!(
                fixture
                    .visual
                    .debug_bounds(Box::leak(
                        format!("device-shell-{}", state.project.next_identity()).into_boxed_str()
                    ))
                    .is_none()
            );
            assert_eq!(
                fs::read(fixture.files.0.join("plugin-catalog.json")).unwrap(),
                fixture.original_catalog,
                "refusal must preserve last-good catalog bytes"
            );
        }
        fixture.visual.update(|window, _| window.remove_window());
        fixture.visual.run_until_parked();
    }
}

#[derive(Clone, Copy, Debug)]
enum Terminal {
    BrowserEscape,
    EditorEscape,
    StaleRevision,
    UnknownPresentation,
}

#[derive(Clone, Copy, Debug)]
enum Closing {
    ViewDrop,
    Discard,
    NewDocument,
}

#[gpui::test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn rendered_native_effect_close_and_document_replacement_cancel_exact_preparation(
    cx: &mut TestAppContext,
) {
    let _native = native_fixture();
    for closing in [Closing::ViewDrop, Closing::Discard, Closing::NewDocument] {
        let mut fixture = open_browser(
            cx,
            2,
            Some(EffectPresentation {
                input_frames: 0,
                output_frames: 512,
            }),
        );
        fixture.activate_to_facts(cx, false);
        let weak = fixture.view(cx).operation.unwrap();
        let operation = weak.upgrade().unwrap();
        for _ in 0..3 {
            repaint(cx, &mut fixture.visual);
            let view = fixture.view(cx);
            assert!(view.pending && !operation.is_cancelled());
            assert_eq!(view.state.as_ref().unwrap().project, fixture.state.project);
            assert!(view.applied.is_none());
        }
        match closing {
            Closing::ViewDrop => {
                fixture.visual.update(|window, _| window.remove_window());
                fixture.visual.run_until_parked();
            }
            Closing::Discard => {
                click(&mut fixture.visual, "window-control-close");
                assert!(cx.has_pending_prompt());
                cx.simulate_prompt_answer("cancel");
                cx.run_until_parked();
                assert!(
                    !operation.is_cancelled(),
                    "cancelled close must leave the real operation pending"
                );
                assert!(fixture.view(cx).pending);
                click(&mut fixture.visual, "window-control-close");
                assert!(cx.has_pending_prompt());
                cx.simulate_prompt_answer("discard");
                cx.run_until_parked();
            }
            Closing::NewDocument => {
                click(&mut fixture.visual, "main-track-header");
                fixture.visual.simulate_keystrokes("cmd-n");
                until("real new-project confirmation", || {
                    repaint(cx, &mut fixture.visual);
                    cx.has_pending_prompt()
                });
                cx.simulate_prompt_answer("don't save");
                cx.run_until_parked();
            }
        }
        until(
            "exact operation cancelled by close or document request",
            || {
                cx.run_until_parked();
                operation.is_cancelled()
            },
        );
        if matches!(closing, Closing::NewDocument) {
            until(
                "fresh document installed after original output handback",
                || {
                    fixture.step(cx);
                    let view = fixture.view(cx);
                    !view.pending
                        && view.state.as_ref().is_some_and(|state| {
                            state.project.tracks().is_empty()
                                && !state.can_undo
                                && !state.can_redo
                                && !state.unsaved
                        })
                        && fixture.records.load(Acquire) == 0
                },
            );
            let view = fixture.view(cx);
            assert!(view.state.unwrap().project.main_devices().is_empty());
            assert!(view.applied.is_none());
            assert!(view.selected_owner.is_none());
            assert_eq!(
                fs::read(fixture.files.0.join("plugin-catalog.json")).unwrap(),
                fixture.original_catalog
            );
            fixture.visual.update(|window, _| window.remove_window());
            fixture.visual.run_until_parked();
        }
        if let Some(mut callback) = fixture.callback.lock().unwrap().take() {
            callback.close();
        }
        drop(operation);
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            cx.run_until_parked();
            if weak.upgrade().is_none() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "{closing:?}: control owners={} windows={} native={:?} records={}",
                weak.strong_count(),
                cx.windows().len(),
                fixture.progress.lock().unwrap(),
                fixture.records.load(Acquire)
            );
            std::thread::yield_now();
        }
        // Existing exclusive output construction proves the old Root handback;
        // no test flag substitutes for native source/worker disposal custody.
        let mut next = ResidentHarness::with_plugins(
            host(),
            Some(EffectPresentation {
                input_frames: 0,
                output_frames: 512,
            }),
        );
        assert!(next.next().error.is_none());
        next.applied(0);
    }
}

#[gpui::test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn rendered_native_effect_terminals_preserve_canonical_history_and_original_source(
    cx: &mut TestAppContext,
) {
    let _native = native_fixture();
    for terminal in [
        Terminal::BrowserEscape,
        Terminal::EditorEscape,
        Terminal::StaleRevision,
        Terminal::UnknownPresentation,
    ] {
        let presentation =
            (!matches!(terminal, Terminal::UnknownPresentation)).then_some(EffectPresentation {
                input_frames: 0,
                output_frames: 512,
            });
        let mut fixture = open_browser(cx, 2, presentation);
        fixture.activate_to_facts(cx, false);
        until(
            "pre-existing publications drained before terminal control",
            || {
                let mut slot = fixture.callback.lock().unwrap();
                let backend = slot.as_mut().unwrap();
                backend
                    .render_native_direct(&mut [[0.0; 2]; 64], &mut [0.0; 128])
                    .unwrap();
                backend.ordinary_publication_idle_on_control() == Some(true)
            },
        );
        let operation = fixture.view(cx).operation.unwrap().upgrade().unwrap();
        match terminal {
            Terminal::BrowserEscape | Terminal::EditorEscape => {
                if matches!(terminal, Terminal::EditorEscape) {
                    click(&mut fixture.visual, "main-track-header");
                }
                fixture.visual.simulate_keystrokes("escape");
                assert!(
                    operation.is_cancelled(),
                    "{terminal:?} must cancel the actual request"
                );
            }
            Terminal::StaleRevision => {
                click(&mut fixture.visual, "main-track-header");
                let before = fixture.view(cx).queued_requests;
                fixture.visual.simulate_keystrokes("cmd-t");
                assert_eq!(
                    fixture.view(cx).queued_requests,
                    before + 1,
                    "the intervening edit must be queued before native preparation resumes"
                );
            }
            Terminal::UnknownPresentation => {}
        }
        if matches!(terminal, Terminal::StaleRevision) {
            until(
                "ordinary edit commits while only native Facts are held",
                || {
                    repaint(cx, &mut fixture.visual);
                    fixture.view(cx).queued_requests == 0
                },
            );
            let intervening = fixture.view(cx);
            assert!(
                intervening.error.is_none(),
                "intervening edit refused: {:?}",
                intervening.error
            );
            assert_eq!(
                intervening.state.as_ref().unwrap().project.tracks().len(),
                2
            );
            assert!(
                intervening
                    .state
                    .as_ref()
                    .unwrap()
                    .project
                    .main_devices()
                    .is_empty()
            );
        }
        // Completed paints preserve the real ordinary edit, and never show a
        // device for an operation cancelled/refused at this held native boundary.
        for _ in 0..3 {
            repaint(cx, &mut fixture.visual);
            let view = fixture.view(cx);
            if matches!(terminal, Terminal::StaleRevision) {
                assert_eq!(
                    view.state.as_ref().unwrap().revision,
                    fixture.state.revision + 1
                );
                assert_eq!(view.state.as_ref().unwrap().project.tracks().len(), 2);
            } else {
                assert_eq!(view.state.as_ref().unwrap().project, fixture.state.project);
                assert_eq!(
                    view.state.as_ref().unwrap().revision,
                    fixture.state.revision
                );
            }
            if matches!(terminal, Terminal::UnknownPresentation) {
                assert!(view.pending);
            }
            assert!(view.applied.is_none());
            assert!(
                fixture
                    .visual
                    .debug_bounds(Box::leak(
                        format!("device-shell-{}", fixture.state.project.next_identity())
                            .into_boxed_str()
                    ))
                    .is_none()
            );
        }
        if matches!(terminal, Terminal::UnknownPresentation) {
            fixture.resume.send(()).unwrap();
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            fixture.step(cx);
            let view = fixture.view(cx);
            if !view.pending && fixture.records.load(Acquire) == 0 {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "{terminal:?}: pending={} records={} native={:?} revision={:?} status={} error={:?} callback={:?}",
                view.pending,
                fixture.records.load(Acquire),
                fixture.progress.lock().unwrap(),
                view.state.as_ref().map(|s| s.revision),
                view.status,
                view.error,
                fixture
                    .callback
                    .lock()
                    .unwrap()
                    .as_ref()
                    .and_then(|b| b.status())
                    .map(|s| (
                        s.applied_revision.load(Acquire),
                        s.rejected_revision(),
                        s.render_fault_snapshot()
                    ))
            );
            std::thread::yield_now();
        }
        let view = fixture.view(cx);
        let state = view.state.unwrap();
        assert!(view.applied.is_none());
        assert!(state.project.main_devices().is_empty());
        assert_eq!(
            state.project.assets().collect::<Vec<_>>(),
            fixture.state.project.assets().collect::<Vec<_>>()
        );
        assert_eq!(
            state.project.track(fixture.track),
            fixture.state.project.track(fixture.track)
        );
        assert_eq!(
            fs::read(fixture.files.0.join("plugin-catalog.json")).unwrap(),
            fixture.original_catalog
        );
        if matches!(terminal, Terminal::StaleRevision) {
            assert_eq!(state.revision, fixture.state.revision + 1);
            assert_eq!(state.project.tracks().len(), 2);
            assert!(
                state
                    .project
                    .tracks()
                    .iter()
                    .all(|track| track.devices().is_empty())
            );
            click(&mut fixture.visual, "undo · cmd+z");
            until("one Undo removes only the intervening track edit", || {
                fixture.step(cx);
                fixture
                    .view(cx)
                    .state
                    .as_ref()
                    .is_some_and(|state| state.revision == fixture.state.revision + 2)
            });
            let restored = fixture.view(cx).state.unwrap();
            assert_eq!(restored.project.tracks(), fixture.state.project.tracks());
            assert_eq!(
                restored.project.main_devices(),
                fixture.state.project.main_devices()
            );
            assert!(restored.can_redo);
        } else {
            assert_eq!(state.project, fixture.state.project);
            assert_eq!(
                (state.revision, state.can_undo, state.can_redo),
                (
                    fixture.state.revision,
                    fixture.state.can_undo,
                    fixture.state.can_redo
                )
            );
            if matches!(terminal, Terminal::UnknownPresentation) {
                assert!(
                    view.error
                        .as_ref()
                        .is_some_and(|error| error.contains("presentation")),
                    "{terminal:?}: {:?}",
                    view.error
                );
            } else {
                assert!(operation.is_cancelled());
                assert!(
                    view.error
                        .as_ref()
                        .is_none_or(|error| error.contains("cancel")),
                    "{terminal:?}: {:?}",
                    view.error
                );
            }
        }
        fixture.visual.update(|window, _| window.remove_window());
        fixture.visual.run_until_parked();
    }
}

#[gpui::test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn rendered_native_effect_browser_enters_actual_track_return_main_after_ack(
    cx: &mut TestAppContext,
) {
    let _native = native_fixture();
    for (location, pointer) in
        (0..3).flat_map(|location| [false, true].map(|pointer| (location, pointer)))
    {
        let mut fixture = open_browser(
            cx,
            location,
            Some(EffectPresentation {
                input_frames: 0,
                output_frames: 512,
            }),
        );
        fixture.activate_to_facts(cx, pointer);
        let BrowserFixture {
            window,
            mut visual,
            state,
            expected_owner,
            track,
            callback,
            queued,
            completed,
            records,
            progress,
            facts: _,
            resume,
            original_catalog,
            module: _,
            files,
        } = fixture;
        // The real worker is held at Facts, before it can queue this endpoint.
        // Drain pre-existing UI/publication traffic through actual callbacks so
        // the following hold isolates this operation's ACK, not a full queue.
        until("pre-existing publication queue drained", || {
            let mut slot = callback.lock().unwrap();
            let backend = slot.as_mut().unwrap();
            backend
                .render_native_direct(&mut [[0.0; 2]; 64], &mut [0.0; 128])
                .unwrap();
            backend.publication_queue_empty_on_control() == Some(true)
        });
        resume.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            repaint(cx, &mut visual);
            let view = cx
                .update(|app| hex_ui::plugin_effect_test_view(window, app))
                .unwrap();
            assert!(
                view.error.is_none() && view.pending,
                "location{location}: pending={}, selected={:?}, status={}, error={:?}",
                view.pending,
                view.selected_owner,
                view.status,
                view.error
            );
            if queued.load(Acquire) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "location{location}: pending={}, selected={:?}, status={}, records={}, native={:?}",
                view.pending,
                view.selected_owner,
                view.status,
                records.load(Acquire),
                progress.lock().unwrap()
            );
            std::thread::yield_now();
        }
        // Hold the actual endpoint install, not a timer or a fake readiness bit.
        for _ in 0..3 {
            repaint(cx, &mut visual);
            let pending = cx
                .update(|app| hex_ui::plugin_effect_test_view(window, app))
                .unwrap();
            assert!(
                pending.pending && pending.error.is_none(),
                "{:?}",
                pending.error
            );
            assert_eq!(pending.state.as_ref().unwrap().project, state.project);
            assert_eq!(pending.state.as_ref().unwrap().revision, state.revision);
            assert!(pending.applied.is_none());
            assert!(
                visual
                    .debug_bounds(Box::leak(
                        format!("device-shell-{}", state.project.next_identity()).into_boxed_str()
                    ))
                    .is_none()
            );
        }
        until("accepted canonical and native source ACK", || {
            callback
                .lock()
                .unwrap()
                .as_mut()
                .unwrap()
                .render_native_direct(&mut [[0.0; 2]; 64], &mut [0.0; 128])
                .unwrap();
            repaint(cx, &mut visual);
            let view = cx
                .update(|app| hex_ui::plugin_effect_test_view(window, app))
                .unwrap();
            assert!(view.error.is_none(), "{:?}", view.error);
            !view.pending
                && view
                    .state
                    .as_ref()
                    .is_some_and(|s| s.revision == state.revision + 1)
                && view
                    .applied
                    .is_some_and(|source| source.context.editor_revision == state.revision + 1)
        });
        let accepted = cx
            .update(|app| hex_ui::plugin_effect_test_view(window, app))
            .unwrap()
            .state
            .unwrap();
        assert_eq!(records.load(Acquire), 1);
        let devices = match expected_owner {
            PluginEffectDestination::Track(id) => accepted.project.track(id).unwrap().devices(),
            PluginEffectDestination::Return(id) => accepted
                .project
                .return_tracks()
                .iter()
                .find(|r| r.id() == id)
                .unwrap()
                .devices(),
            PluginEffectDestination::Instrument(_) => panic!("effect fixture destination"),
            PluginEffectDestination::Main => accepted.project.main_devices(),
        };
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].id().value(), state.project.next_identity());
        assert_eq!(
            accepted.project.track(track).unwrap().clips(),
            state.project.track(track).unwrap().clips()
        );
        assert_eq!(
            accepted.project.assets().collect::<Vec<_>>(),
            state.project.assets().collect::<Vec<_>>()
        );
        assert_eq!(
            fs::read(files.0.join("plugin-catalog.json")).unwrap(),
            original_catalog
        );
        let device = state.project.next_identity();
        assert!(
            visual
                .debug_bounds(Box::leak(
                    format!("device-vst3-active-{device}").into_boxed_str()
                ))
                .is_some(),
            "accepted Track/Return/Main effect status must follow its actual applied graph"
        );
        assert!(
            visual
                .debug_bounds(Box::leak(format!("device-shell-{device}").into_boxed_str()))
                .is_some(),
            "accepted effect must be painted in its selected chain"
        );
        // One UI Undo removes exactly the insertion; Redo restores the same
        // authored state and original source, not a newly loaded substitute.
        for (action, revision, project, native) in [
            ("undo · cmd+z", state.revision + 2, &state.project, false),
            (
                "redo · cmd+shift+z",
                state.revision + 3,
                &accepted.project,
                true,
            ),
        ] {
            click(&mut visual, action);
            until(action, || {
                callback
                    .lock()
                    .unwrap()
                    .as_mut()
                    .unwrap()
                    .render_native_direct(&mut [[0.0; 2]; 64], &mut [0.0; 128])
                    .unwrap();
                repaint(cx, &mut visual);
                let view = cx
                    .update(|app| hex_ui::plugin_effect_test_view(window, app))
                    .unwrap();
                assert!(
                    view.error.is_none(),
                    "location{location} pointer={pointer} {action}: {:?}",
                    view.error
                );
                view.state
                    .as_ref()
                    .is_some_and(|state| state.revision == revision)
                    && (!native
                        || view
                            .applied
                            .is_some_and(|source| source.context.editor_revision == revision))
            });
            let restored = cx
                .update(|app| hex_ui::plugin_effect_test_view(window, app))
                .unwrap()
                .state
                .unwrap();
            // Project deliberately keeps a monotonic identity watermark across
            // Undo. Authored tracks, buses, device state and media must return
            // exactly; allocated IDs must never become available for reuse.
            assert_eq!(restored.project.tracks(), project.tracks());
            assert_eq!(restored.project.return_tracks(), project.return_tracks());
            assert_eq!(restored.project.main_devices(), project.main_devices());
            assert_eq!(
                restored.project.assets().collect::<Vec<_>>(),
                project.assets().collect::<Vec<_>>()
            );
            assert_eq!(
                restored.project.next_identity(),
                accepted.project.next_identity()
            );
            if native {
                assert_eq!(restored.project, accepted.project);
            }
            assert_eq!(records.load(Acquire), 1);
            assert_eq!(
                visual
                    .debug_bounds(Box::leak(format!("device-shell-{device}").into_boxed_str()))
                    .is_some(),
                native
            );
        }
        click(&mut visual, "play or pause · space");
        repaint(cx, &mut visual);
        let started = cx
            .update(|app| hex_ui::plugin_effect_test_view(window, app))
            .unwrap();
        assert!(started.error.is_none(), "play refused: {:?}", started.error);
        let mut output = Vec::new();
        for block in 0..10 {
            let mut pcm = [[0.0; 2]; 256];
            let mut host_pcm = [0.0; 512];
            let allocations = {
                let mut slot = callback.lock().unwrap();
                allocation_counter::measure(|| {
                    slot.as_mut()
                        .unwrap()
                        .render_native_direct(&mut pcm, &mut host_pcm)
                        .unwrap()
                })
            };
            assert_eq!(
                (
                    allocations.count_total,
                    allocations.bytes_total,
                    allocations.count_current,
                    allocations.bytes_current
                ),
                (0, 0, 0, 0)
            );
            assert!(host_pcm.chunks_exact(2).zip(&pcm).all(|(h, p)| h == p));
            output.extend(pcm);
            until("original DSP request completion", || {
                completed.load(Acquire) > block
            });
        }
        assert!(output[..512].iter().all(|frame| *frame == [0.0; 2]));
        for (index, frame) in output.iter().enumerate().skip(512) {
            let scale = 0.5 + ((index - 512) / 256) as f32 * 0.125;
            let expected = if location == 1 {
                0.25 + 0.125 * scale
            } else {
                0.25 * scale
            };
            assert_eq!(
                *frame, [expected; 2],
                "original worker PCM at location{location}, frame{index}"
            );
        }
        if location == 0 && !pointer {
            let selector = Box::leak(format!("device-vst3-editor-{device}").into_boxed_str());
            click(&mut visual, selector);
            let deadline = Instant::now() + Duration::from_secs(3);
            until("editor response from the original SDK controller", || {
                repaint(cx, &mut visual);
                let view = cx
                    .update(|app| hex_ui::plugin_effect_test_view(window, app))
                    .unwrap();
                if let Some(error) = view.error.as_deref() {
                    assert_eq!(error, "native editor unavailable for this vst3");
                    return true;
                }
                assert!(
                    Instant::now() < deadline,
                    "editor click had no terminal: status={:?}, queued={}",
                    view.status,
                    view.queued_requests
                );
                false
            });
            let after = cx
                .update(|app| hex_ui::plugin_effect_test_view(window, app))
                .unwrap();
            assert_eq!(after.state.unwrap().project, accepted.project);
            assert_eq!(records.load(Acquire), 1);
        }
        visual.update(|window, _| window.remove_window());
        visual.run_until_parked();
        drop(visual);
        drop(callback);
    }
}

#[gpui::test]
#[ignore = "requires source-bound SDK receipts with native fixture variant 115"]
fn rendered_vst3_instrument_drop_targets_the_instrument_slot_and_shows_editor_toggle(
    cx: &mut TestAppContext,
) {
    let _native = native_fixture();
    let mut fixture = open_browser_metadata(
        cx,
        0,
        Some(EffectPresentation {
            input_frames: 0,
            output_frames: 512,
        }),
        "Instrument|Synth",
        0,
        true,
    );
    let track = fixture.track;
    let anchor = fixture.state.project.track(track).unwrap().devices()[0].id();
    let start = fixture
        .visual
        .debug_bounds("browser-vst3-row-0")
        .unwrap()
        .center();
    let card = fixture
        .visual
        .debug_bounds(Box::leak(
            format!("device-drop-card-{}", anchor.value()).into_boxed_str(),
        ))
        .unwrap()
        .center();
    let modifiers = Modifiers::default();
    fixture
        .visual
        .simulate_mouse_down(start, MouseButton::Left, modifiers);
    fixture.visual.simulate_mouse_move(
        gpui::point(start.x + gpui::px(12.0), start.y),
        MouseButton::Left,
        modifiers,
    );
    fixture
        .visual
        .simulate_mouse_move(card, MouseButton::Left, modifiers);
    repaint(cx, &mut fixture.visual);
    assert!(!fixture.view(cx).pending);
    fixture
        .visual
        .simulate_mouse_up(card, MouseButton::Left, modifiers);
    until("instrument drop reached native facts", || {
        repaint(cx, &mut fixture.visual);
        let view = fixture.view(cx);
        assert!(view.error.is_none(), "{:?}", view.error);
        view.pending && fixture.facts.try_recv().is_ok()
    });
    fixture.resume.send(()).unwrap();
    until("instrument graph applied", || {
        fixture.step(cx);
        let view = fixture.view(cx);
        assert!(view.error.is_none(), "{:?}", view.error);
        !view.pending
            && view
                .applied
                .is_some_and(|source| source.context.editor_revision == fixture.state.revision + 1)
    });
    let accepted = fixture.view(cx).state.unwrap();
    let track = accepted.project.track(track).unwrap();
    let instrument = track.instrument().unwrap();
    assert_eq!(
        instrument.id().value(),
        fixture.state.project.next_identity()
    );
    assert_eq!(track.devices()[0].id(), anchor);
    assert_eq!(track.devices().len(), 1);
    let selector =
        Box::leak(format!("device-vst3-editor-{}", instrument.id().value()).into_boxed_str());
    until("instrument editor button rendered", || {
        repaint(cx, &mut fixture.visual);
        fixture.visual.debug_bounds(selector).is_some()
    });
    fixture.visual.update(|window, _| window.remove_window());
    fixture.visual.run_until_parked();
}

#[gpui::test]
#[ignore = "explicit original SDK helper/source receipts required"]
fn live_vst3_pointer_drag_reaches_native_device_during_playback(cx: &mut gpui::TestAppContext) {
    const CHILD: &str = "HEX_DESKTOP_TEST_NATIVE_PARAMETER_POINTER";
    if std::env::var_os(CHILD).is_none() {
        let files = Files::new();
        let name = std::thread::current().name().unwrap().to_owned();
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &name, "--ignored", "--nocapture"])
            .env(CHILD, "1")
            .env("HEX_ORIGINAL_INSTANCE_VARIANT", "37")
            .env("HEX_ORIGINAL_RESTORE_TRACE", files.0.join("restore-trace"))
            .env("HOME", files.0.join("home"))
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        return;
    }
    let _native = native_fixture();
    let mut fixture = open_browser(
        cx,
        2,
        Some(EffectPresentation {
            input_frames: 0,
            output_frames: 512,
        }),
    );
    fixture.activate_to_facts(cx, false);
    fixture.resume.send(()).unwrap();
    let revision = fixture.state.revision + 1;
    until("original VST3 inserted and applied", || {
        fixture.step(cx);
        let view = fixture.view(cx);
        !view.pending
            && view
                .applied
                .is_some_and(|source| source.context.editor_revision == revision)
    });
    let initial = fixture.view(cx).state.unwrap().project.clone();
    let device = initial.main_devices()[0].id();
    let callback = Arc::clone(&fixture.callback);
    let window = fixture.window;
    let mut visual = fixture.visual;
    let view = |cx: &mut gpui::TestAppContext| {
        cx.update(|app| hex_ui::plugin_effect_test_view(window, app))
            .unwrap()
    };
    let render_pcm = || {
        let mut pcm = [[0.0; 2]; 256];
        callback
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .render_native_direct(&mut pcm, &mut [0.0; 512])
            .unwrap();
        pcm
    };
    let render = || {
        let pcm = render_pcm();
        wait(|| {
            callback
                .lock()
                .unwrap()
                .as_mut()
                .unwrap()
                .native_workers_idle_on_control()
                == Some(true)
        });
        pcm
    };
    click(&mut visual, "main-track-header");
    let knob = Box::leak(format!("vst3-knob-{}-7-knob", device.value()).into_boxed_str());
    until("actual controller knob paints automatically", || {
        repaint(cx, &mut visual);
        assert!(view(cx).error.is_none(), "{:?}", view(cx).error);
        visual.debug_bounds(knob).is_some()
    });
    click(&mut visual, "play or pause · space");
    for _ in 0..3 {
        render();
    }
    // Admit a real background page immediately before user input. Its reply
    // must neither make the gesture busy nor replace the captured control.
    until("background parameter page is in flight", || {
        repaint(cx, &mut visual);
        view(cx).parameter_query_pending
    });
    let control_identity = view(cx)
        .parameter_controls
        .into_iter()
        .find_map(|(current, parameter, entity)| {
            (current == device.value() && parameter == 7).then_some(entity)
        })
        .expect("painted controller identity");
    let start = visual.debug_bounds(knob).unwrap().center();
    let end = gpui::point(start.x, start.y - gpui::px(40.0));
    let mods = gpui::Modifiers::default();
    visual.simulate_mouse_down(start, gpui::MouseButton::Left, mods);
    repaint(cx, &mut visual);
    visual.simulate_mouse_move(end, gpui::MouseButton::Left, mods);
    repaint(cx, &mut visual);
    let mut peak: f32 = 0.0;
    for _ in 0..6 {
        std::thread::sleep(Duration::from_millis(5));
        let pcm = render();
        peak = peak.max(pcm.iter().map(|frame| frame[0].abs()).fold(0.0, f32::max));
        repaint(cx, &mut visual);
        assert!(
            view(cx)
                .parameter_controls
                .iter()
                .any(|&(current, parameter, entity)| {
                    current == device.value() && parameter == 7 && entity == control_identity
                }),
            "a completed repaint must retain the held parameter control"
        );
    }
    assert!(
        peak > 0.30,
        "original PCM while mouse is still held: {peak}"
    );
    assert_eq!(view(cx).state.unwrap().project, initial);
    visual.simulate_mouse_up(end, gpui::MouseButton::Left, mods);
    until("pointer release commits one VST3 edit", || {
        render();
        repaint(cx, &mut visual);
        let view = view(cx);
        assert!(view.error.is_none(), "{:?}", view.error);
        view.state.is_some_and(|state| {
            state.revision == revision + 1
                && state.project.device(device).unwrap().state()
                    != initial.device(device).unwrap().state()
        })
    });

    let before_stop = view(cx).applied.unwrap().audio_revision;
    let before_edit = view(cx).state.unwrap().project.clone();
    click(&mut visual, "stop · escape");
    let deadline = Instant::now() + Duration::from_secs(3);
    until("stopped restore refreshes the open controller list", || {
        assert!(render_pcm().iter().all(|sample| *sample == [0.0; 2]));
        repaint(cx, &mut visual);
        let view = view(cx);
        assert!(view.error.is_none(), "{:?}", view.error);
        let ready = view
            .applied
            .is_some_and(|source| source.audio_revision > before_stop)
            && visual.debug_bounds(knob).is_some();
        assert!(
            ready || Instant::now() < deadline,
            "restore status: {:?}; applied: {:?}; before: {before_stop}; knob: {:?}; restoring: {}; retry: {}; pending: {}; queued: {}",
            view.status,
            view.applied,
            visual.debug_bounds(knob),
            visual.debug_bounds("vst3-controls-restoring").is_some(),
            visual.debug_bounds("vst3-controls-retry").is_some(),
            view.pending,
            view.queued_requests
        );
        ready
    });
    let start = visual.debug_bounds(knob).unwrap().center();
    let end = gpui::point(start.x, start.y + gpui::px(20.0));
    visual.simulate_mouse_down(start, gpui::MouseButton::Left, mods);
    repaint(cx, &mut visual);
    visual.simulate_mouse_move(end, gpui::MouseButton::Left, mods);
    repaint(cx, &mut visual);
    visual.simulate_mouse_up(end, gpui::MouseButton::Left, mods);
    until("stopped pointer release commits a second VST3 edit", || {
        assert!(render_pcm().iter().all(|sample| *sample == [0.0; 2]));
        repaint(cx, &mut visual);
        let view = view(cx);
        assert!(view.error.is_none(), "{:?}", view.error);
        view.state.is_some_and(|state| {
            state.revision == revision + 2
                && state.project.device(device).unwrap().state()
                    != before_edit.device(device).unwrap().state()
        })
    });
}

#[gpui::test]
#[ignore = "explicit original SDK helper/source receipts required"]
fn native_editor_touch_updates_host_follow_knob_and_arranger(cx: &mut gpui::TestAppContext) {
    const CHILD: &str = "HEX_DESKTOP_TEST_NATIVE_TOUCH";
    if std::env::var_os(CHILD).is_none() {
        let files = Files::new();
        let name = std::thread::current().name().unwrap().to_owned();
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &name, "--ignored", "--nocapture"])
            .env(CHILD, "1")
            .env("HEX_ORIGINAL_INSTANCE_VARIANT", "147")
            .env("HEX_ORIGINAL_INSTANCE_RELEASE", files.0.join("release"))
            .env("HOME", files.0.join("home"))
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        return;
    }
    let _native = native_fixture();
    let mut fixture = open_browser(
        cx,
        2,
        Some(EffectPresentation {
            input_frames: 0,
            output_frames: 512,
        }),
    );
    fixture.activate_to_facts(cx, false);
    fixture.resume.send(()).unwrap();
    let revision = fixture.state.revision + 1;
    until("original VST3 inserted and applied", || {
        fixture.step(cx);
        let view = fixture.view(cx);
        !view.pending
            && view
                .applied
                .is_some_and(|source| source.context.editor_revision == revision)
    });
    let initial = fixture.view(cx).state.unwrap().project.clone();
    let device = initial.main_devices()[0].id().value();
    click(&mut fixture.visual, "main-track-header");
    until("automatic host parameter control", || {
        fixture.step(cx);
        fixture
            .view(cx)
            .parameter_controls
            .iter()
            .any(|&(id, parameter, _)| id == device && parameter == 7)
    });
    assert!(fixture.view(cx).parameter_touches.is_empty());
    let release = std::env::var_os("HEX_ORIGINAL_INSTANCE_RELEASE").unwrap();
    fs::write(release, b"emit original SDK begin/perform/end callbacks").unwrap();
    until(
        "native receipt refreshes the follow control and arranger lane while stopped",
        || {
            fixture.step(cx);
            let view = fixture.view(cx);
            assert!(view.error.is_none(), "{:?}", view.error);
            view.parameter_touches
                .iter()
                .any(|&(id, parameter, value, control, lane)| {
                    id == device
                        && parameter == 7
                        && (value - 0.75).abs() < 1e-8
                        && control.is_some()
                        && lane
                })
        },
    );
    let view = fixture.view(cx);
    let follow = view.parameter_touches[0].3.unwrap();
    let row = view
        .parameter_controls
        .iter()
        .find(|&&(id, parameter, _)| id == device && parameter == 7)
        .unwrap()
        .2;
    assert_ne!(row, follow, "one control entity cannot be painted twice");
    assert_eq!(
        view.state.unwrap().project,
        initial,
        "observation must not author automation or canonical state"
    );
    let selector = Box::leak(format!("vst3-follow-knob-{device}-7-knob").into_boxed_str());
    let start = fixture
        .visual
        .debug_bounds(selector)
        .expect("painted follow knob")
        .center();
    let end = gpui::point(start.x, start.y + gpui::px(30.0));
    let modifiers = Modifiers::default();
    fixture
        .visual
        .simulate_mouse_down(start, MouseButton::Left, modifiers);
    repaint(cx, &mut fixture.visual);
    fixture
        .visual
        .simulate_mouse_move(end, MouseButton::Left, modifiers);
    repaint(cx, &mut fixture.visual);
    fixture
        .visual
        .simulate_mouse_up(end, MouseButton::Left, modifiers);
    until(
        "follow knob uses the existing canonical gesture contract",
        || {
            fixture.step(cx);
            let view = fixture.view(cx);
            assert!(view.error.is_none(), "{:?}", view.error);
            view.state
                .as_ref()
                .is_some_and(|state| state.revision == revision + 1 && state.project != initial)
        },
    );
    until(
        "last touched selection survives its canonical commit and fresh source",
        || {
            fixture.step(cx);
            fixture.view(cx).parameter_touches.iter().any(
                |&(id, parameter, value, control, lane)| {
                    id == device && parameter == 7 && value < 0.74 && control.is_some() && lane
                },
            )
        },
    );
    fixture.visual.update(|window, _| window.remove_window());
    fixture.visual.run_until_parked();
}
