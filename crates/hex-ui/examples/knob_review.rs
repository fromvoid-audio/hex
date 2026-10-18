//! Disposable native visual fixture: no audio device, project or settings I/O.
//! Run `cargo run -p hex-ui --example knob_review`, then compare/control the
//! isolated window. All entities use the production shared widget renderer.
#[path = "../src/theme.rs"]
#[allow(dead_code)]
mod theme;

use gpui::{
    App, AppContext, Application, Bounds, Context, Entity, Window, WindowBounds, WindowOptions,
    div, prelude::*, px, rgb, size,
};
use hex_widgets::{
    ControlStyle, DialLayout, NumericControl, NumericControlAppearance, NumericControlConfig,
    NumericControlKind, NumericScale, NumericSpec,
};

struct KnobReview {
    rows: Vec<(&'static str, Vec<Entity<NumericControl>>)>,
}

fn style() -> ControlStyle {
    ControlStyle {
        background: theme::color(theme::PaletteRole::Background),
        surface: theme::color(theme::PaletteRole::Surface),
        raised: theme::color(theme::PaletteRole::Raised),
        rail: theme::color(theme::PaletteRole::Border),
        border: theme::color(theme::PaletteRole::Border),
        text: theme::color(theme::PaletteRole::Text),
        muted: theme::color(theme::PaletteRole::Muted),
        accent: theme::color(theme::PaletteRole::Secondary),
        focus_marker: 0xaaaaaa,
        dial_rail: 0x000000,
        hover: theme::color(theme::PaletteRole::Hover),
        font_size: 9.0,
        text_case: hex_widgets::TextCase::Lowercase,
    }
}

impl KnobReview {
    fn new(cx: &mut App) -> Result<Self, String> {
        let mut rows = Vec::new();
        for (index, (title, label, appearance, layout, domain)) in [
            (
                "micro knob · ratio",
                "coarse",
                NumericControlAppearance::MicroDial,
                DialLayout::ReadoutGap,
                (0.5, 48.0, NumericScale::Linear),
            ),
            (
                "compact knob · frequency",
                "freq",
                NumericControlAppearance::CompactDial,
                DialLayout::ReadoutGap,
                (20.0, 20_000.0, NumericScale::Logarithmic),
            ),
            (
                "pan knob · standard density",
                "balance",
                NumericControlAppearance::Standard,
                DialLayout::Pan,
                (-100.0, 100.0, NumericScale::Bipolar),
            ),
            (
                "bottom gap knob · compact density",
                "send",
                NumericControlAppearance::CompactDial,
                DialLayout::BottomGap,
                (0.0, 100.0, NumericScale::Linear),
            ),
            (
                "mini knob · amount",
                "amount",
                NumericControlAppearance::MiniDial,
                DialLayout::ReadoutGap,
                (0.0, 100.0, NumericScale::Linear),
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let mut controls = Vec::new();
            let spec = NumericSpec::new(
                domain.0,
                domain.1,
                if layout == DialLayout::Pan {
                    0.0
                } else {
                    domain.0
                },
                domain.2,
                2,
                None::<String>,
            )
            .map_err(|error| error.to_string())?;
            for (column, position) in [0.0, 0.5, 1.0].into_iter().enumerate() {
                let value = spec
                    .from_normalized(position)
                    .map_err(|error| error.to_string())?;
                let config = NumericControlConfig::new(
                    format!("review-{index}-{column}"),
                    theme::text(label),
                    NumericControlKind::Knob,
                    spec.clone(),
                    value,
                    style(),
                )
                .map_err(|error| error.to_string())?
                .with_appearance(appearance)
                .with_dial_layout(layout);
                controls.push(cx.new(|cx| NumericControl::from_config(cx, config)));
            }
            rows.push((title, controls));
        }
        Ok(Self { rows })
    }
}

impl Render for KnobReview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let mut root = div()
            .size_full()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .p(px(12.0))
            .bg(rgb(style().surface))
            .font_family(theme::FONT_FAMILY)
            .text_size(px(10.0))
            .text_color(rgb(style().text))
            .child(theme::text("hex · shared knob review"))
            .child(theme::text(
                "initial columns: normalized minimum / middle / maximum",
            ));
        for (title, controls) in &self.rows {
            root = root.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(theme::text(title))
                    .child(
                        div()
                            .flex()
                            .gap(px(18.0))
                            .children(controls.iter().cloned()),
                    ),
            );
        }
        root.child(theme::text(
            "each column is editable · click / drag / type / escape",
        ))
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        if let Err(error) = theme::load_fonts(cx) {
            eprintln!("{error}");
            cx.quit();
            return;
        }
        hex_widgets::bind_numeric_control_keys(cx);
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        let review = match KnobReview::new(cx) {
            Ok(review) => review,
            Err(error) => {
                eprintln!("{error}");
                cx.quit();
                return;
            }
        };
        let result = cx.open_window(
            WindowOptions {
                titlebar: Some(gpui::TitlebarOptions {
                    title: Some(theme::text("hex · knob review")),
                    ..Default::default()
                }),
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                    None,
                    size(px(520.0), px(520.0)),
                    cx,
                ))),
                window_min_size: Some(size(px(340.0), px(520.0))),
                ..Default::default()
            },
            move |_, cx| cx.new(|_| review),
        );
        if let Err(error) = result {
            eprintln!("could not open knob review: {error}");
            cx.quit();
        }
        cx.activate(true);
    });
}
