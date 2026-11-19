//! Exercise the real recorder header in an offline rendered editor window.
use crate::editor::rendered_test_fixture::{audio_project, recorder_window};
use crate::theme;
use gpui::{Modifiers, TestAppContext, VisualTestContext, px, rgb};
use hex_audio::sample_recording::Phase;

pub(crate) fn exercise(cx: &mut TestAppContext, root: std::path::PathBuf) {
    #[cfg(target_os = "macos")]
    {
        cx.use_native_text_system().expect("native font shaping");
        cx.update(|cx| {
            // Register the same embedded faces without enumerating every
            // installed system font. Each shaped run is checked below.
            let fonts: Vec<std::borrow::Cow<'static, [u8]>> = vec![
                std::borrow::Cow::Borrowed(include_bytes!(
                    "../../../../../assets/fonts/ioskeley-mono/ioskeley-mono-regular.ttf"
                )),
                std::borrow::Cow::Borrowed(include_bytes!(
                    "../../../../../assets/fonts/ioskeley-mono/ioskeley-mono-medium.ttf"
                )),
                std::borrow::Cow::Borrowed(include_bytes!(
                    "../../../../../assets/fonts/ioskeley-mono/ioskeley-mono-bold.ttf"
                )),
                std::borrow::Cow::Borrowed(include_bytes!(
                    "../../../../../assets/fonts/ioskeley-mono/ioskeley-mono-italic.ttf"
                )),
            ];
            cx.text_system().add_fonts(fonts).expect("bundled fonts");
        });
    }
    let (project, _, _, _) = audio_project();
    let (window, editor) = recorder_window(cx, project, root);
    let mut visual = VisualTestContext::from_window(window, cx);
    let check = |visual: &mut VisualTestContext, selector, label: &str| {
        let text = theme::text(label);
        let width = visual.update(|window, _| {
            let text_system = window.text_system();
            let shaped = text_system.shape_line(
                text.clone(),
                window.rem_size() * 0.75,
                &[gpui::TextRun {
                    len: text.len(),
                    font: gpui::font(theme::FONT_FAMILY),
                    color: rgb(0xffffff).into(),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                }],
                None,
            );
            #[cfg(target_os = "macos")]
            {
                assert!(!shaped.runs.is_empty(), "label must have shaped glyphs");
                for run in &shaped.runs {
                    let font = text_system
                        .get_font_for_id(run.font_id)
                        .expect("shaped font must be identified");
                    assert_eq!(font.family.as_ref(), theme::FONT_FAMILY);
                }
            }
            shaped.width
        });
        let bounds = visual.debug_bounds(selector).expect("painted control");
        assert!(bounds.size.width > width, "{label} must fit its button");
        bounds
    };
    for (width, height) in [(880.0, 540.0), (1440.0, 800.0)] {
        visual.simulate_resize(gpui::size(px(width), px(height)));
        visual.run_until_parked();
        let mode = check(&mut visual, "sample-recorder-mode", "recorder");
        visual.simulate_click(mode.center(), Modifiers::default());
        visual.run_until_parked();
        let panel = visual.debug_bounds("sample-recorder").unwrap();
        for (phase, label) in [(Some(Phase::Capturing), "stop"), (None, "record")] {
            editor.update(&mut visual, |view, cx| {
                view.sample_recorder.phase = phase;
                cx.notify();
            });
            visual.run_until_parked();
            let mode = check(&mut visual, "sample-recorder-mode", "spectrum");
            let record = check(&mut visual, "sample-record", label);
            assert!(panel.left() <= mode.left() && mode.right() < record.left());
            assert!(record.right() <= panel.right());
        }
        let record = visual.debug_bounds("sample-record").unwrap();
        visual.simulate_click(record.center(), Modifiers::default());
        editor.read_with(&visual, |view, _| {
            assert_eq!(
                view.sample_recorder.error.as_deref(),
                Some("arm at least one source for sampling")
            );
        });
        let mode = visual.debug_bounds("sample-recorder-mode").unwrap();
        visual.simulate_click(mode.center(), Modifiers::default());
        visual.run_until_parked();
        assert!(visual.debug_bounds("project-spectrum").is_some());
    }
    visual.update(|window, _| window.remove_window());
    visual.run_until_parked();
}
