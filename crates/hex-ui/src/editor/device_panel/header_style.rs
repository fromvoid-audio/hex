//! Pure device-panel header labels, source tooltips, and drop-bounds geometry.
use super::source_summary;
use crate::theme;
use gpui::{Bounds, Div, Pixels, div, prelude::*, rgb};

pub(super) fn title_label(title: String, warning: bool) -> Div {
    let preferred = theme::color(if warning {
        theme::PaletteRole::Primary
    } else {
        theme::PaletteRole::Muted
    });
    div()
        .flex_1()
        .min_w_0()
        .truncate()
        .text_color(rgb(theme::readable_text(
            theme::color(theme::PaletteRole::Surface),
            preferred,
        )))
        .child(theme::text(title))
}

pub(super) fn retained_source_path_label(track: &hex_project::Track) -> Option<&'static str> {
    track
        .retained_source_path()
        .then_some("source path · printed audio bypasses")
}

pub(super) fn retained_source_path_badge(label: &'static str) -> Div {
    let background = theme::color(theme::PaletteRole::Surface);
    div()
        .debug_selector(|| "retained-source-path".into())
        .flex_shrink()
        .min_w_0()
        .truncate()
        .text_color(rgb(theme::readable_text(
            background,
            theme::color(theme::PaletteRole::SecondaryText),
        )))
        .child(theme::text(label))
}

pub(super) fn source_summary_tooltip(summary: &source_summary::SourceSummary) -> String {
    let receiver = summary
        .receiver
        .as_deref()
        .map(hex_session::service::text::verbatim)
        .unwrap_or_else(|| "none".into());
    format!(
        "source · {}\nrole · {}\nreceiver · {receiver}\nmidi tracks · {}",
        hex_session::service::text::verbatim(&summary.name),
        summary.role,
        summary.contributors,
    )
}

pub(super) fn pointer_within_visible_device_drop_bounds(
    pointer: &gpui::Point<Pixels>,
    target: &Bounds<Pixels>,
    visible_chain: Option<&Bounds<Pixels>>,
) -> bool {
    target.contains(pointer) && visible_chain.is_some_and(|bounds| bounds.contains(pointer))
}

#[cfg(all(test, feature = "ui-unit-tier"))]
mod tests {
    use super::*;
    use gpui::px;
    use hex_project::{Command, Created, Editor, Project};

    struct RestoreTheme(hex_preferences::ThemePreferences);
    impl Drop for RestoreTheme {
        fn drop(&mut self) {
            theme::apply_preferences(self.0);
        }
    }

    fn luminance(color: gpui::Rgba) -> f32 {
        let linear = |c: f32| {
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * linear(color.r) + 0.7152 * linear(color.g) + 0.0722 * linear(color.b)
    }

    #[test]
    fn clipped_device_card_bounds_cannot_claim_offscreen_pointer() {
        let viewport = gpui::Bounds::new(
            gpui::point(px(200.0), px(300.0)),
            gpui::size(px(400.0), px(100.0)),
        );
        let clipped_card = gpui::Bounds::new(
            gpui::point(px(100.0), px(300.0)),
            gpui::size(px(286.0), px(100.0)),
        );
        let inside_both = gpui::point(px(250.0), px(330.0));
        let offscreen_card_area = gpui::point(px(150.0), px(330.0));
        let outside_rail = gpui::point(px(250.0), px(420.0));

        assert!(pointer_within_visible_device_drop_bounds(
            &inside_both,
            &clipped_card,
            Some(&viewport),
        ));
        assert!(!pointer_within_visible_device_drop_bounds(
            &offscreen_card_area,
            &clipped_card,
            Some(&viewport),
        ));
        assert!(!pointer_within_visible_device_drop_bounds(
            &outside_rail,
            &clipped_card,
            Some(&viewport),
        ));
    }

    #[test]
    fn source_tooltip_preserves_full_authored_names() {
        let summary = source_summary::SourceSummary {
            name: "A Very Long Bright Bass Name".into(),
            receiver: Some("MIDI Receiver A".into()),
            contributors: 2,
            role: "midi contributor",
        };
        assert_eq!(
            theme::text(source_summary_tooltip(&summary)).as_ref(),
            "source · A Very Long Bright Bass Name\nrole · midi contributor\nreceiver · MIDI Receiver A\nmidi tracks · 2"
        );
    }

    #[test]
    fn device_panel_title_render_style_meets_text_contrast_without_changing_palette() {
        let _restore = RestoreTheme(theme::preferences());
        let colors = hex_preferences::ThemePreferences {
            background: hex_preferences::RgbColor::new(48, 48, 48),
            primary: hex_preferences::RgbColor::new(121, 121, 121),
            text: hex_preferences::RgbColor::new(255, 255, 255),
            secondary: hex_preferences::RgbColor::new(255, 255, 255),
        };
        assert!(
            theme::colors_are_legible(colors),
            "valid existing palette policy"
        );
        theme::apply_preferences(colors);
        for warning in [false, true] {
            // Inspect the actual Div style supplied to both rendered owner headers.
            let mut label = title_label("finish the active device value first".into(), warning);
            let foreground = label.text_style().as_ref().unwrap().color.unwrap().to_rgb();
            let background = rgb(theme::color(theme::PaletteRole::Surface));
            let first = luminance(foreground);
            let second = luminance(background);
            let contrast = (first.max(second) + 0.05) / (first.min(second) + 0.05);
            assert!(
                contrast >= 4.5,
                "warning={warning}, actual rendered title contrast={contrast}"
            );
            assert_eq!(theme::preferences(), colors);
        }
    }

    #[test]
    fn retained_receiver_labels_the_existing_source_path_and_printed_audio_bypass() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Track(receiver) = editor
            .execute(Command::InsertTrackWithDevice {
                name: "receiver".into(),
                before_track: None,
                slot: hex_project::TrackDeviceSlot::Instrument,
                state: hex_project::default_instrument_state().unwrap(),
            })
            .unwrap()
        else {
            panic!("receiver")
        };
        let ordinary = editor.project().track(receiver).unwrap();
        assert_eq!(retained_source_path_label(ordinary), None);
        let instrument = ordinary.instrument().unwrap().id();
        let source = ordinary.source_id();
        let mut snapshot = editor.project().snapshot();
        snapshot.retained_source_paths.push(receiver.value());
        let project = Project::from_snapshot(snapshot).unwrap();
        let retained = project.track(receiver).unwrap();
        let label = retained_source_path_label(retained).unwrap();
        assert_eq!(label, "source path · printed audio bypasses");
        assert_eq!(retained.instrument().unwrap().id(), instrument);
        assert_eq!(retained.source_id(), source);
        let mut badge = retained_source_path_badge(label);
        let foreground = badge.text_style().as_ref().unwrap().color.unwrap().to_rgb();
        let background = rgb(theme::color(theme::PaletteRole::Surface));
        let first = luminance(foreground);
        let second = luminance(background);
        assert!((first.max(second) + 0.05) / (first.min(second) + 0.05) >= 4.5);
    }
}
