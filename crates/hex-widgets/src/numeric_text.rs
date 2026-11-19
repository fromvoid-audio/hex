use crate::{ControlStyle, numeric_control::NumericControl};
use gpui::{
    App, Bounds, Context, Element, ElementId, ElementInputHandler, Entity, EntityInputHandler,
    GlobalElementId, LayoutId, MouseDownEvent, PaintQuad, Pixels, Point, ShapedLine, Style,
    TextRun, UTF16Selection, UnderlineStyle, Window, fill, point, prelude::*, px, relative, rgb,
    size,
};
use std::ops::Range;

const MAX_NUMERIC_TEXT_BYTES: usize = 256;
// A 127-unit VST UTF-16 string may require up to 381 UTF-8 bytes.
const MAX_DEFERRED_TEXT_BYTES: usize = 512;

pub(crate) struct NumericTextElement {
    pub(crate) input: Entity<NumericControl>,
}

impl EntityInputHandler for NumericControl {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = range_utf16_to_bytes(&self.text, range);
        actual.replace(range_bytes_to_utf16(&self.text, range.clone()));
        self.text.get(range).map(str::to_owned)
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: range_bytes_to_utf16(&self.text, self.selected.clone()),
            reversed: self.reversed,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked
            .as_ref()
            .map(|range| range_bytes_to_utf16(&self.text, range.clone()))
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.caret_activity = self.caret_activity.wrapping_add(1);
        cx.notify();
        self.marked = None;
        self.invalidate_text_pointer_selection();
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        new_text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.caret_activity = self.caret_activity.wrapping_add(1);
        cx.notify();
        self.invalidate_text_pointer_selection();
        let new_text = new_text.replace(['\n', '\r'], "");
        let range = range
            .map(|range| range_utf16_to_bytes(&self.text, range))
            .or(self.marked.clone())
            .unwrap_or_else(|| self.selected.clone());
        let Some(selected) = self.text.get(range.clone()) else {
            return;
        };
        if self
            .text
            .len()
            .saturating_sub(selected.len())
            .checked_add(new_text.len())
            .is_none_or(|length| {
                length
                    > if self.deferred_text_enabled() {
                        MAX_DEFERRED_TEXT_BYTES
                    } else {
                        MAX_NUMERIC_TEXT_BYTES
                    }
            })
        {
            self.invalid_text = true;
            cx.notify();
            return;
        }
        let changed = !selected.is_empty() || !new_text.is_empty();
        if changed {
            self.invalidate_pending_text();
        }
        self.text_modified |= changed;
        self.text.replace_range(range.clone(), &new_text);
        let cursor = range.start + new_text.len();
        self.selected = cursor..cursor;
        self.reversed = false;
        self.marked = None;
        self.invalid_text = false;
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        new_text: &str,
        new_selected: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.caret_activity = self.caret_activity.wrapping_add(1);
        cx.notify();
        self.invalidate_text_pointer_selection();
        let new_text = new_text.replace(['\n', '\r'], "");
        let range = range
            .map(|range| range_utf16_to_bytes(&self.text, range))
            .or(self.marked.clone())
            .unwrap_or_else(|| self.selected.clone());
        let Some(selected) = self.text.get(range.clone()) else {
            return;
        };
        if self
            .text
            .len()
            .saturating_sub(selected.len())
            .checked_add(new_text.len())
            .is_none_or(|length| {
                length
                    > if self.deferred_text_enabled() {
                        MAX_DEFERRED_TEXT_BYTES
                    } else {
                        MAX_NUMERIC_TEXT_BYTES
                    }
            })
        {
            self.invalid_text = true;
            cx.notify();
            return;
        }
        let changed = !selected.is_empty() || !new_text.is_empty();
        if changed {
            self.invalidate_pending_text();
        }
        self.text_modified |= changed;
        self.text.replace_range(range.clone(), &new_text);
        self.marked = (!new_text.is_empty()).then_some(range.start..range.start + new_text.len());
        self.selected = new_selected
            .map(|selected| range_utf16_to_bytes(&new_text, selected))
            .map(|selected| range.start + selected.start..range.start + selected.end)
            .unwrap_or_else(|| range.start + new_text.len()..range.start + new_text.len());
        self.reversed = false;
        self.invalid_text = false;
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let line = self.last_layout.as_ref()?;
        let range = range_utf16_to_bytes(&self.text, range);
        let origin_x = self.last_origin_x.unwrap_or(bounds.left());
        Some(Bounds::from_corners(
            point(
                origin_x
                    + line.x_for_index(
                        self.value_text_case()
                            .display_offset(&self.text, range.start),
                    ),
                bounds.top(),
            ),
            point(
                origin_x
                    + line
                        .x_for_index(self.value_text_case().display_offset(&self.text, range.end)),
                bounds.bottom(),
            ),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let bounds = self.last_bounds?;
        let origin_x = self.last_origin_x.unwrap_or(bounds.left());
        let index = self.last_layout.as_ref()?.index_for_x(point.x - origin_x)?;
        let index = self.value_text_case().source_offset(&self.text, index);
        Some(byte_offset_to_utf16(&self.text, index))
    }
}

pub struct TextPaintState {
    line: ShapedLine,
    origin_x: Pixels,
    cursor: Option<PaintQuad>,
    selection: Option<PaintQuad>,
}

impl IntoElement for NumericTextElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for NumericTextElement {
    type RequestLayoutState = ();
    type PrepaintState = Option<TextPaintState>;

    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.0).into();
        style.size.height = window.line_height().into();
        (window.request_layout(style, [], cx), ())
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let input = self.input.read(cx);
        let text = input.value_string();
        let style = window.text_style();
        let color = rgb(input.style.text).into();
        let run = TextRun {
            len: text.len(),
            font: style.font(),
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let marked = input.marked.as_ref().map(|range| {
            input
                .value_text_case()
                .display_offset(&input.text, range.start)
                ..input
                    .value_text_case()
                    .display_offset(&input.text, range.end)
        });
        let caret_run = TextRun {
            len: 1,
            ..run.clone()
        };
        let runs = if let Some(marked) = marked {
            vec![
                TextRun {
                    len: marked.start,
                    ..run.clone()
                },
                TextRun {
                    len: marked.end - marked.start,
                    underline: Some(UnderlineStyle {
                        color: Some(color),
                        thickness: px(1.0),
                        wavy: false,
                    }),
                    ..run.clone()
                },
                TextRun {
                    len: text.len() - marked.end,
                    ..run
                },
            ]
            .into_iter()
            .filter(|run| run.len > 0)
            .collect()
        } else {
            vec![run]
        };
        let line = window.text_system().shape_line(
            text.into(),
            style.font_size.to_pixels(window.rem_size()),
            &runs,
            None,
        );
        let cell_width = window
            .text_system()
            .shape_line(
                "m".into(),
                style.font_size.to_pixels(window.rem_size()),
                &[caret_run],
                None,
            )
            .width
            .max(px(1.0));
        let cursor_x = line.x_for_index(
            input
                .value_text_case()
                .display_offset(&input.text, input.cursor()),
        );
        let max_x = (bounds.size.width - cell_width).max(px(0.0));
        let shift = if cursor_x > max_x {
            cursor_x - max_x
        } else if cursor_x < px(0.0) {
            cursor_x
        } else {
            px(0.0)
        };
        let origin_x = bounds.left() - shift;
        let selection = (!input.selected.is_empty()).then(|| {
            fill(
                Bounds::from_corners(
                    point(
                        origin_x
                            + line.x_for_index(
                                input
                                    .value_text_case()
                                    .display_offset(&input.text, input.selected.start),
                            ),
                        bounds.top(),
                    ),
                    point(
                        origin_x
                            + line.x_for_index(
                                input
                                    .value_text_case()
                                    .display_offset(&input.text, input.selected.end),
                            ),
                        bounds.bottom(),
                    ),
                ),
                rgb(selection_tint(input.style)),
            )
        });
        let cursor = (input.editing.is_editing() && input.selected.is_empty()).then(|| {
            fill(
                Bounds::new(
                    point(origin_x + cursor_x, bounds.top()),
                    size(
                        cell_width.min(bounds.size.width),
                        bounds.bottom() - bounds.top(),
                    ),
                ),
                rgb(input.style.accent),
            )
        });
        Some(TextPaintState {
            line,
            origin_x,
            cursor,
            selection,
        })
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        state: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(state) = state.take() else {
            return;
        };
        let focus = self.input.read(cx).focus.clone();
        if self.input.read(cx).editing.is_editing() {
            let input = self.input.clone();
            window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                if phase.capture() && !bounds.contains(&event.position) {
                    input.update(cx, |control, cx| control.finish_text_edit_on_blur(cx));
                    window.refresh();
                }
            });
            window.handle_input(
                &focus,
                ElementInputHandler::new(bounds, self.input.clone()),
                cx,
            );
        }
        if let Some(selection) = state.selection {
            window.paint_quad(selection);
        }
        if focus.is_focused(window)
            && let Some(cursor) = state.cursor
        {
            let activity = self.input.read(cx).caret_activity;
            crate::text_caret::paint(cursor, self.input.entity_id(), activity, window, cx);
        }
        let _ = state.line.paint(
            point(state.origin_x, bounds.top()),
            window.line_height(),
            window,
            cx,
        );
        self.input.update(cx, |input, _| {
            input.last_layout = Some(state.line);
            input.last_bounds = Some(bounds);
            input.last_origin_x = Some(state.origin_x);
        });
    }
}

fn selection_tint(style: ControlStyle) -> u32 {
    const OPACITY: f32 = 0.4;
    let blend = |accent: u32, surface: u32, shift: u32| {
        let accent = ((accent >> shift) & 0xff) as f32;
        let surface = ((surface >> shift) & 0xff) as f32;
        (accent * OPACITY + surface * (1.0 - OPACITY)).round() as u32
    };
    (blend(style.accent, style.surface, 16) << 16)
        | (blend(style.accent, style.surface, 8) << 8)
        | blend(style.accent, style.surface, 0)
}

fn range_utf16_to_bytes(text: &str, range: Range<usize>) -> Range<usize> {
    utf16_to_byte(text, range.start)..utf16_to_byte(text, range.end)
}

fn range_bytes_to_utf16(text: &str, range: Range<usize>) -> Range<usize> {
    byte_offset_to_utf16(text, range.start)..byte_offset_to_utf16(text, range.end)
}

fn utf16_to_byte(text: &str, offset: usize) -> usize {
    let mut units = 0;
    for (index, character) in text.char_indices() {
        if units >= offset {
            return index;
        }
        units += character.len_utf16();
    }
    text.len()
}

fn byte_offset_to_utf16(text: &str, offset: usize) -> usize {
    text.char_indices()
        .take_while(|(index, _)| *index < offset)
        .map(|(_, character)| character.len_utf16())
        .sum()
}

#[cfg(test)]
mod tests {
    use super::selection_tint;
    use crate::ControlStyle;

    fn style(accent: u32) -> ControlStyle {
        ControlStyle {
            background: 0x202020,
            surface: 0x303030,
            raised: 0x383838,
            rail: 0x282828,
            border: 0x505050,
            text: 0xe0e0e0,
            muted: 0xaaaaaa,
            accent,
            accent_text: 0x202020,
            numeric_fill: 0x31515a,
            hover_text: 0xe0e0e0,
            focus_marker: 0xaaaaaa,
            dial_rail: 0x000000,
            hover: 0x404040,
            selection: 0x3350a0,
            invalid: 0xff554c,
            font_size: 10.0,
            text_case: crate::TextCase::Preserve,
        }
    }

    #[test]
    fn numeric_text_selection_tint_tracks_the_injected_secondary_accent() {
        assert_ne!(
            selection_tint(style(0x55ddee)),
            selection_tint(style(0xee8844))
        );
        assert_ne!(selection_tint(style(0x55ddee)), style(0x303030).surface);
    }
}
