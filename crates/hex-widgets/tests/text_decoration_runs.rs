//! Shaped lines keep every decoration run across inline-storage boundaries.
//!
//! GPUI stores a line's decoration runs inline up to a fixed capacity and spills
//! beyond it. Shape and paint lines whose run counts sit on both sides of the
//! inline boundary, with mixed UTF-8, so a storage change cannot drop, reorder or
//! fail to paint any styled span.
use gpui::{
    Bounds, Context, Hsla, IntoElement, ParentElement, Render, SharedString, StrikethroughStyle,
    Styled, TestAppContext, TextRun, UnderlineStyle, VisualTestContext, Window, canvas, div, font,
    point, px, size,
};

const RUN_COUNTS: [usize; 6] = [1, 4, 5, 32, 33, 80];
static PAINTED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn styled_runs(text: &str, count: usize) -> Vec<TextRun> {
    let boundaries: Vec<usize> = text.char_indices().map(|(i, _)| i).collect();
    let chars = boundaries.len();
    (0..count)
        .map(|index| {
            let start = boundaries[index * chars / count];
            let end = boundaries
                .get((index + 1) * chars / count)
                .copied()
                .unwrap_or(text.len());
            let hue = index as f32 / count as f32;
            TextRun {
                len: end - start,
                font: font("Ioskeley Mono"),
                color: Hsla {
                    h: hue,
                    s: 0.8,
                    l: 0.5,
                    a: 1.0,
                },
                background_color: (index % 2 == 0).then_some(Hsla {
                    h: hue,
                    s: 0.3,
                    l: 0.2,
                    a: 1.0,
                }),
                underline: (index % 3 == 0).then_some(UnderlineStyle {
                    thickness: px(1.0),
                    color: Some(Hsla {
                        h: hue,
                        s: 1.0,
                        l: 0.7,
                        a: 1.0,
                    }),
                    wavy: index % 2 == 1,
                }),
                strikethrough: (index % 5 == 0).then_some(StrikethroughStyle {
                    thickness: px(1.0),
                    color: None,
                }),
            }
        })
        .collect()
}

fn label(count: usize) -> String {
    // Mixed one- to four-byte characters; at least one character per run.
    "aé✓𝄞".repeat(count.div_ceil(4) + 1)
}

struct Lines;
impl Render for Lines {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(
            canvas(
                |_, _, _| (),
                |bounds: Bounds<gpui::Pixels>, _, window, cx| {
                    for (row, count) in RUN_COUNTS.into_iter().enumerate() {
                        let text = label(count);
                        let runs = styled_runs(&text, count);
                        let line = window.text_system().shape_line(
                            SharedString::from(text.clone()),
                            px(11.0),
                            &runs,
                            None,
                        );
                        assert_eq!(line.len(), text.len(), "{count} runs");
                        assert_eq!(line.text.as_ref(), text, "{count} runs");
                        let origin =
                            point(bounds.origin.x, bounds.origin.y + px(14.0 * row as f32));
                        line.paint_background(origin, px(14.0), window, cx).unwrap();
                        line.paint(origin, px(14.0), window, cx).unwrap();
                        PAINTED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                },
            )
            .size_full(),
        )
    }
}

#[gpui::test]
fn decoration_runs_survive_inline_storage_boundaries(cx: &mut TestAppContext) {
    for count in RUN_COUNTS {
        let text = label(count);
        let runs = styled_runs(&text, count);
        assert_eq!(runs.len(), count);
        assert_eq!(runs.iter().map(|run| run.len).sum::<usize>(), text.len());
        assert!(runs.iter().all(|run| run.len > 0), "{count} runs");
    }
    let window = cx.add_window(|_, _| Lines);
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    visual.simulate_resize(size(px(800.0), px(200.0)));
    visual.run_until_parked();
    visual.update(|window, _| window.refresh());
    visual.run_until_parked();
    assert!(
        PAINTED.load(std::sync::atomic::Ordering::Relaxed) >= RUN_COUNTS.len(),
        "every line must actually shape and paint"
    );
}
