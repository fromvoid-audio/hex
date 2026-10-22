use super::*;
use gpui::{AppContext, Modifiers, MouseButton, TestAppContext, VisualTestContext};
use hex_session::{
    CachedPeak, CachedWaveform,
    service::{Service, ViewState},
};
use std::sync::Arc;
const CHOICES: [&str; 6] = [
    "audio-clip-warp-mode-0",
    "audio-clip-warp-mode-1",
    "audio-clip-warp-mode-2",
    "audio-clip-warp-mode-3",
    "audio-clip-warp-mode-4",
    "audio-clip-warp-mode-5",
];

#[gpui::test]
fn selected_clip_warp_toggle_is_visible_and_dispatches_transient_beats(cx: &mut TestAppContext) {
    let (handle, editor, _, clip, paths) = bounded_audio_detail_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    paths.assert_bounded(&editor, cx);
    let toggle = visual
        .debug_bounds("audio-clip-warp-toggle")
        .expect("visible warp toggle");
    let sidebar = visual.debug_bounds("arrangement-audio-controls").unwrap();
    assert!(toggle.origin.y >= sidebar.origin.y && toggle.bottom() <= sidebar.bottom());
    visual.simulate_mouse_down(toggle.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(toggle.center(), MouseButton::Left, Modifiers::default());
    editor.read_with(cx, |view, _| {
        assert!(matches!(&view.last_enqueued_edit, Some(Command::SetAudioClipWarp { clip: id, settings: Some(settings) })
            if *id == clip && settings.mode == modes()[0]));
    });
}
#[gpui::test]
fn algorithm_picker_dispatches_direct_choice_without_resetting_current_settings(
    cx: &mut TestAppContext,
) {
    assert_mode_choice(cx, 0);
    assert_mode_choice(cx, 1);
}
fn assert_mode_choice(cx: &mut TestAppContext, requested: usize) {
    let (handle, editor, _, clip, paths) = bounded_audio_detail_fixture(cx);
    prepare_beats_fixture(&editor, clip, cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    paths.assert_bounded(&editor, cx);
    let sidebar = visual.debug_bounds("arrangement-audio-controls").unwrap();
    for selector in CHOICES {
        let bounds = visual
            .debug_bounds(selector)
            .expect("implemented algorithm choice");
        assert!(bounds.origin.y >= sidebar.origin.y && bounds.bottom() <= sidebar.bottom());
    }
    // Re-selecting Beats preserves the user's decay instead of resetting defaults.
    let bounds = visual.debug_bounds(CHOICES[requested]).unwrap();
    visual.simulate_mouse_down(bounds.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(bounds.center(), MouseButton::Left, Modifiers::default());
    editor.read_with(cx, |view, _| {
        assert!(matches!(&view.last_enqueued_edit,
        Some(Command::SetAudioClipWarp{clip:id,settings:Some(settings)})
        if *id == clip && if requested == 0 { matches!(settings.mode,WarpMode::Beats{decay:60,..}) }
            else { settings.mode == modes()[requested] }))
    });
}

fn prepare_beats_fixture(editor: &gpui::Entity<EditorView>, clip: ClipId, cx: &mut TestAppContext) {
    editor.update(cx, |view, cx| {
        let old = view.state.as_ref().unwrap();
        let mut model = hex_project::Editor::new((*old.project).clone(), 8).unwrap();
        model
            .execute(command(model.project(), clip, Action::Warp).unwrap())
            .unwrap();
        model
            .execute(command(model.project(), clip, Action::Decay(-40)).unwrap())
            .unwrap();
        view.state = Some(Arc::new(ViewState {
            canonical_audio_source: None,
            comparison_audio: None,
            project: Arc::new(model.project().clone()),
            waveforms: old.waveforms.clone(),
            source_overviews: old.source_overviews.clone(),
            audio_clip_waveforms: old.audio_clip_waveforms.clone(),
            midi_preview: old.midi_preview.clone(),
            gain_preview: old.gain_preview.clone(),
            timeline_frames: old.timeline_frames,
            can_undo: true,
            can_redo: false,
            revision: old.revision + 1,
            unsaved: true,
            document_path: None,
        }));
        cx.notify();
    });
}

#[gpui::test]
fn selected_clip_beats_controls_are_visible_and_dispatch_existing_loop_envelope_bindings(
    cx: &mut TestAppContext,
) {
    for (selector, expected_tail, expected_decay) in [
        ("audio-clip-beats-stop", BeatsTail::Off, 60),
        ("audio-clip-beats-forward", BeatsTail::Forward, 60),
        ("audio-clip-beats-alternating", BeatsTail::Alternating, 60),
        ("audio-clip-beats-envelope-less", BeatsTail::Forward, 50),
        ("audio-clip-beats-envelope-more", BeatsTail::Forward, 70),
    ] {
        let (handle, editor, _, clip, paths) = bounded_audio_detail_fixture(cx);
        prepare_beats_fixture(&editor, clip, cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        paths.assert_bounded(&editor, cx);
        let sidebar = visual.debug_bounds("arrangement-audio-controls").unwrap();
        for name in [
            "audio-clip-beats-preserve",
            "audio-clip-beats-stop",
            "audio-clip-beats-forward",
            "audio-clip-beats-alternating",
            "audio-clip-beats-envelope",
        ] {
            let bounds = visual.debug_bounds(name).expect("visible Beats binding");
            assert!(
                bounds.origin.y >= sidebar.origin.y && bounds.bottom() <= sidebar.bottom(),
                "{name} must not be hidden below source fields"
            );
            assert!(
                bounds.origin.x >= sidebar.origin.x && bounds.right() <= sidebar.right(),
                "{name} must fit the sidebar"
            );
        }
        let bounds = visual.debug_bounds(selector).unwrap();
        visual.simulate_mouse_down(bounds.center(), MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(bounds.center(), MouseButton::Left, Modifiers::default());
        editor.read_with(cx, |view, _| {
            assert!(matches!(&view.last_enqueued_edit,
                Some(Command::SetAudioClipWarp { clip: id, settings: Some(settings) })
                if *id == clip && matches!(settings.mode, WarpMode::Beats { tail, decay, .. } if tail == expected_tail && decay == expected_decay)));
        });
    }
}

/// This fixture owns every discovery/preferences path before workers start.
/// It never changes HOME or system fonts, and cannot scan an ambient library.
struct BeatsTestPaths {
    root: std::path::PathBuf,
}
impl BeatsTestPaths {
    fn new() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "hex-beats-controls-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(root.join("samples")).unwrap();
        Self { root }
    }
    fn assert_bounded(&self, editor: &gpui::Entity<EditorView>, cx: &TestAppContext) {
        editor.read_with(cx, |view, _| {
            assert_eq!(view.browser.directory, self.root.join("samples"));
            assert_eq!(view.browser.places.clone(), vec![self.root.join("samples")]);
        });
    }
}
impl Drop for BeatsTestPaths {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn bounded_audio_detail_fixture(
    cx: &mut TestAppContext,
) -> (
    gpui::AnyWindowHandle,
    gpui::Entity<EditorView>,
    hex_project::TrackId,
    ClipId,
    BeatsTestPaths,
) {
    let paths = BeatsTestPaths::new();
    let (project, track, _, clip) = super::super::clip_drag_ui_test::audio_project();
    let asset = project.clip(clip).unwrap().asset();
    let peaks = Arc::new(
        CachedWaveform::from_peaks(
            96_000,
            vec![
                [
                    CachedPeak {
                        min: -0.6,
                        max: 0.7
                    },
                    CachedPeak {
                        min: -0.4,
                        max: 0.5
                    },
                ];
                64
            ],
        )
        .unwrap(),
    );
    // GPUI TestPlatform deliberately uses NoopTextSystem, whose font list is
    // empty even after add_fonts. These are scene/input tests, not native font
    // registration evidence; do not change system fonts or fake that proof.

    let bootstrap = cx.add_window(|_, _| gpui::Empty);
    let handle = *bootstrap;
    let editor = cx
        .update_window(handle, |_, window, cx| {
            let service = Service::new(48_000, 32, 8_000_000).unwrap();
            let editor = window.replace_root(cx, |window, cx| {
                EditorView::new_for_test_with_browser_directory(window, cx, service, &paths.root)
            });
            super::super::install_close_guard(window, cx, editor.downgrade());
            editor.update(cx, |view, cx| {
                // Constructor received these exact paths before discovery, rather
                // than swapping visible fields after a default worker had started.
                assert_eq!(view.browser.directory, paths.root.join("samples"));
                assert_eq!(
                    view.browser.places.clone(),
                    vec![paths.root.join("samples")]
                );
                view.state = Some(Arc::new(ViewState {
                    canonical_audio_source: None,
                    comparison_audio: None,
                    project: Arc::new(project),
                    waveforms: std::collections::BTreeMap::from([(asset, peaks)]),
                    source_overviews: Default::default(),
                    audio_clip_waveforms: Default::default(),
                    midi_preview: Arc::default(),
                    gain_preview: Ok(Arc::new(Default::default())),
                    timeline_frames: 384_000,
                    can_undo: false,
                    can_redo: false,
                    revision: 1,
                    unsaved: false,
                    document_path: None,
                }));
                view.mode = super::super::EditorMode::Arrangement;
                view.selected_track = Some(track);
                view.selected_clip = Some(clip);
                view.clip_view_open = true;
                cx.notify();
            });
            editor
        })
        .unwrap();
    (handle, editor, track, clip, paths)
}
