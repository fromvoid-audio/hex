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
    for (enabled, width) in [
        (false, 880.0),
        (true, 880.0),
        (false, 1_280.0),
        (true, 1_280.0),
    ] {
        let (handle, editor, _, clip, paths) = bounded_audio_detail_fixture(cx);
        if enabled {
            prepare_beats_fixture(&editor, clip, cx);
        }
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.simulate_resize(gpui::size(px(width), px(540.0)));
        visual.run_until_parked();
        paths.assert_bounded(&editor, cx);
        let toggle = visual
            .debug_bounds("audio-clip-warp-toggle")
            .expect("visible warp toggle");
        let sidebar = visual.debug_bounds("arrangement-audio-controls").unwrap();
        assert!(toggle.origin.y >= sidebar.origin.y && toggle.bottom() <= sidebar.bottom());
        assert!(toggle.origin.x >= sidebar.origin.x && toggle.right() <= sidebar.right());
        assert!(
            toggle.size.width >= px(32.0) && toggle.size.width <= px(64.0),
            "warp label needs a compact readable button: {toggle:?}"
        );
        assert!(
            visual
                .debug_bounds("audio-clip-warp-toggle-indicator")
                .is_none(),
            "warp must use the labeled button rather than an unlabeled square"
        );
        let style = theme::control_style();
        let expected: gpui::Background =
            rgb(if enabled { style.accent } else { style.surface }).into();
        assert_eq!(
            visual.rendered_background("audio-clip-warp-toggle"),
            Some(expected)
        );
        visual.simulate_mouse_down(toggle.center(), MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(toggle.center(), MouseButton::Left, Modifiers::default());
        editor.read_with(cx, |view, _| {
            if enabled {
                assert!(matches!(&view.last_enqueued_edit, Some(Command::SetAudioClipWarp { clip: id, settings: None }) if *id == clip));
            } else {
                assert!(matches!(&view.last_enqueued_edit, Some(Command::SetAudioClipWarp { clip: id, settings: Some(settings) }) if *id == clip && settings.mode == modes()[0]));
            }
        });
    }
}
#[gpui::test]
fn selected_long_clip_warp_toggle_enables_tones(cx: &mut TestAppContext) {
    use hex_project::{Created, Editor, Project};
    let mut model = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Imported { track, clip, .. } = model
        .execute(Command::ImportAudioTrack {
            name: "long".into(),
            path: "long.wav".into(),
            sample_rate: 48_000,
            frames: 20_000_000,
        })
        .unwrap()
    else {
        panic!("fixture import");
    };
    model
        .execute(Command::SetClipFades {
            clip,
            fade_in: 0,
            fade_out: 0,
        })
        .unwrap();
    let (handle, editor) =
        super::super::clip_drag_ui_test::editor_window(cx, model.project().clone(), Arc::default());
    editor.update(cx, |view, cx| {
        view.selected_track = Some(track);
        view.selected_clip = Some(clip);
        view.clip_view_open = true;
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let toggle = visual.debug_bounds("audio-clip-warp-toggle").unwrap();
    visual.simulate_click(toggle.center(), Modifiers::default());
    let edit = editor.read_with(cx, |view, _| view.last_enqueued_edit.clone().unwrap());
    assert!(matches!(&edit,
        Command::SetAudioClipWarp { clip: id, settings: Some(settings) }
        if *id == clip && matches!(settings.mode, WarpMode::Tones { .. })
    ));
    model.execute(edit).unwrap();
    let result = model.project().clip(clip).unwrap();
    assert_eq!(result.duration(), 20_000_000);
    assert!(result.audio_edit().unwrap().domain.warp_enabled);
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

#[gpui::test]
fn shift_selected_warp_applies_to_every_clip_with_its_own_source_region(cx: &mut TestAppContext) {
    let (handle, editor, track, first, _paths) = bounded_audio_detail_fixture(cx);
    let (mut model, second) = editor.read_with(cx, |view, _| {
        let mut model =
            hex_project::Editor::new((*view.state.as_ref().unwrap().project).clone(), 16).unwrap();
        let asset = model.project().clip(first).unwrap().asset();
        let hex_project::Created::Clip(second) = model
            .execute(Command::PlaceClip {
                track,
                asset,
                start: 144_000,
                duration: 24_000,
                source_offset: 12_000,
                source_length: 24_000,
                looping: false,
            })
            .unwrap()
        else {
            panic!("clip")
        };
        model
            .execute(Command::SetAudioClipPitch {
                clip: second,
                cents: 700,
            })
            .unwrap();
        (model, second)
    });
    editor.update(cx, |view, cx| {
        let old = view.state.as_ref().unwrap();
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
            can_undo: false,
            can_redo: false,
            revision: old.revision + 1,
            unsaved: false,
            document_path: None,
        }));
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    for (id, modifiers) in [
        (first, Modifiers::default()),
        (
            second,
            Modifiers {
                shift: true,
                ..Modifiers::default()
            },
        ),
    ] {
        let selector: &'static str =
            Box::leak(format!("clip-name-{}", id.value()).into_boxed_str());
        let bounds = visual
            .debug_bounds(selector)
            .expect("rendered selectable header");
        visual.simulate_mouse_down(bounds.center(), MouseButton::Left, modifiers);
        visual.simulate_mouse_up(bounds.center(), MouseButton::Left, modifiers);
        visual.run_until_parked();
    }
    editor.read_with(cx, |view, _| {
        assert_eq!(view.selected_clip_ids(), vec![first, second])
    });
    // Other scalar controls share dispatch; raw source/geometry commands may
    // not silently target only the focused member of a whole-clip selection.
    editor.read_with(cx, |view, _| {
        use hex_session::service::Request;
        let Request::Edit(relative) = view
            .resolve_clip_action_scope(
                Request::Edit(
                    actions::selection_command(model.project(), second, Action::Pitch(100))
                        .unwrap(),
                ),
                false,
            )
            .unwrap()
        else {
            panic!("relative edit")
        };
        let mut pitched = hex_project::Editor::new(model.project().clone(), 8).unwrap();
        pitched.execute(relative).unwrap();
        assert_eq!(
            pitched
                .project()
                .clip(first)
                .unwrap()
                .audio_edit()
                .unwrap()
                .domain
                .pitch_cents,
            100
        );
        assert_eq!(
            pitched
                .project()
                .clip(second)
                .unwrap()
                .audio_edit()
                .unwrap()
                .domain
                .pitch_cents,
            800
        );
        let request = view
            .resolve_clip_action_scope(
                Request::Edit(Command::SetAudioClipGain {
                    clip: second,
                    millidb: -6_000,
                }),
                false,
            )
            .unwrap();
        let Request::Edit(command) = request else {
            panic!("edit")
        };
        let mut gain = hex_project::Editor::new(model.project().clone(), 8).unwrap();
        gain.execute(command).unwrap();
        for id in [first, second] {
            assert_eq!(gain.project().clip(id).unwrap().gain().millidb(), -6_000);
        }
        assert!(
            view.resolve_clip_action_scope(
                Request::Edit(Command::TrimClipEdge {
                    clip: second,
                    edge: hex_project::TrimEdge::Left,
                    frame: 145_000,
                }),
                false
            )
            .is_err()
        );
        assert!(
            view.resolve_clip_action_scope(
                Request::Edit(Command::SetAudioClipWarp {
                    clip: second,
                    settings: None,
                }),
                false
            )
            .is_err()
        );
        assert!(
            view.resolve_clip_action_scope(
                Request::Edit(Command::SetAudioClipWarp {
                    clip: second,
                    settings: None,
                }),
                true
            )
            .is_ok()
        );
    });
    let bounds = visual.debug_bounds("audio-clip-warp-toggle").unwrap();
    visual.simulate_mouse_down(bounds.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(bounds.center(), MouseButton::Left, Modifiers::default());
    let command = editor.read_with(cx, |view, _| view.last_enqueued_edit.clone().unwrap());
    let before = model.project().clone();
    model.execute(command).unwrap();
    for id in [first, second] {
        let clip = model.project().clip(id).unwrap();
        let domain = &clip
            .audio_edit()
            .expect("selected clip must receive warp")
            .domain;
        assert!(domain.warp_enabled);
        let warp = domain.warp.as_ref().unwrap();
        assert_eq!(warp.mode, modes()[0]);
        assert_eq!(
            warp.markers[0].source_frame,
            before.clip(id).unwrap().source_offset()
        );
    }
    assert!(model.undo());
    assert_eq!(model.project(), &before);
}
