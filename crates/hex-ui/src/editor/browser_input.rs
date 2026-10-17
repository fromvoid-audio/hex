//! Native GPUI text input used by the browser filter, including IME and clipboard.
use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, Element, ElementId, ElementInputHandler,
    Entity, EntityInputHandler, FocusHandle, Focusable, GlobalElementId, LayoutId, MouseButton,
    MouseDownEvent, PaintQuad, Pixels, Point, ShapedLine, Style, TextRun, UTF16Selection,
    UnderlineStyle, Window, div, fill, point, prelude::*, px, relative, rgba, size,
};
use std::ops::Range;

gpui::actions!(
    hex_browser_search,
    [
        BrowserBackspace,
        BrowserDelete,
        BrowserLeft,
        BrowserRight,
        BrowserSelectLeft,
        BrowserSelectRight,
        BrowserSelectAll,
        BrowserHome,
        BrowserEnd,
        BrowserPaste,
        BrowserCopy,
        BrowserCut,
    ]
);

pub(super) struct BrowserSearch {
    pub content: String,
    focus: FocusHandle,
    selected: Range<usize>,
    reversed: bool,
    marked: Option<Range<usize>>,
    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    last_origin_x: Option<Pixels>,
}

impl BrowserSearch {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            content: String::new(),
            focus: cx.focus_handle(),
            selected: 0..0,
            reversed: false,
            marked: None,
            last_layout: None,
            last_bounds: None,
            last_origin_x: None,
        }
    }

    pub(super) fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }
    pub(super) fn focus(&self, window: &mut Window) {
        self.focus.focus(window);
    }
    pub(super) fn focus_and_select_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.focus(window);
        self.select_all(&BrowserSelectAll, window, cx);
    }

    pub(super) fn clear(&mut self, cx: &mut Context<Self>) {
        self.content.clear();
        self.selected = 0..0;
        self.reversed = false;
        self.marked = None;
        cx.notify();
    }

    fn cursor(&self) -> usize {
        if self.reversed {
            self.selected.start
        } else {
            self.selected.end
        }
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.content[..offset.min(self.content.len())]
            .char_indices()
            .last()
            .map_or(0, |(index, _)| index)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        let offset = offset.min(self.content.len());
        self.content[offset..]
            .char_indices()
            .nth(1)
            .map_or(self.content.len(), |(index, _)| offset + index)
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        let offset = offset.min(self.content.len());
        if self.reversed {
            self.selected.start = offset;
        } else {
            self.selected.end = offset;
        }
        if self.selected.end < self.selected.start {
            self.reversed = !self.reversed;
            self.selected = self.selected.end..self.selected.start;
        }
        cx.notify();
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        let offset = offset.min(self.content.len());
        self.selected = offset..offset;
        self.reversed = false;
        cx.notify();
    }

    fn replace(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.replace_text_in_range(None, text, window, cx);
    }

    fn backspace(&mut self, _: &BrowserBackspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected.is_empty() {
            let start = self.previous_boundary(self.cursor());
            self.selected = start..self.cursor();
        }
        self.replace("", window, cx);
    }

    fn delete(&mut self, _: &BrowserDelete, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected.is_empty() {
            let end = self.next_boundary(self.cursor());
            self.selected = self.cursor()..end;
        }
        self.replace("", window, cx);
    }

    fn left(&mut self, _: &BrowserLeft, _: &mut Window, cx: &mut Context<Self>) {
        let target = if self.selected.is_empty() {
            self.previous_boundary(self.cursor())
        } else {
            self.selected.start
        };
        self.move_to(target, cx);
    }

    fn right(&mut self, _: &BrowserRight, _: &mut Window, cx: &mut Context<Self>) {
        let target = if self.selected.is_empty() {
            self.next_boundary(self.cursor())
        } else {
            self.selected.end
        };
        self.move_to(target, cx);
    }

    fn select_left(&mut self, _: &BrowserSelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor()), cx);
    }

    fn select_right(&mut self, _: &BrowserSelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor()), cx);
    }

    fn select_all(&mut self, _: &BrowserSelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.selected = 0..self.content.len();
        self.reversed = false;
        cx.notify();
    }

    fn home(&mut self, _: &BrowserHome, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }

    fn end(&mut self, _: &BrowserEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.content.len(), cx);
    }

    fn paste(&mut self, _: &BrowserPaste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            let clean = text.replace(['\n', '\r'], " ");
            self.replace(&clean, window, cx);
        }
    }

    fn copy(&mut self, _: &BrowserCopy, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.content.get(self.selected.clone()) {
            if !text.is_empty() {
                cx.write_to_clipboard(ClipboardItem::new_string(text.to_owned()));
            }
        }
    }

    fn cut(&mut self, _: &BrowserCut, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.content.get(self.selected.clone()) {
            if !text.is_empty() {
                cx.write_to_clipboard(ClipboardItem::new_string(text.to_owned()));
                self.replace("", window, cx);
            }
        }
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf16 = 0;
        let mut utf8 = 0;
        for ch in self.content.chars() {
            if utf16 >= offset {
                break;
            }
            utf16 += ch.len_utf16();
            utf8 += ch.len_utf8();
        }
        utf8
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        self.content[..offset.min(self.content.len())]
            .encode_utf16()
            .count()
    }

    fn range_from_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range.start)..self.offset_from_utf16(range.end)
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn index_for_point(&self, point: Point<Pixels>) -> usize {
        let (Some(bounds), Some(line)) = (self.last_bounds, self.last_layout.as_ref()) else {
            return 0;
        };
        if point.y < bounds.top() {
            return 0;
        }
        if point.y > bounds.bottom() {
            return self.content.len();
        }
        line.closest_index_for_x(point.x - self.last_origin_x.unwrap_or(bounds.left()))
    }

    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.modifiers.shift {
            self.select_to(self.index_for_point(event.position), cx);
        } else {
            self.move_to(self.index_for_point(event.position), cx);
        }
        self.focus.focus(window);
    }
}

impl EntityInputHandler for BrowserSearch {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range);
        actual.replace(self.range_to_utf16(&range));
        self.content.get(range).map(str::to_owned)
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected),
            reversed: self.reversed,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.as_ref().map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked = None;
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range
            .as_ref()
            .map(|range| self.range_from_utf16(range))
            .or_else(|| self.marked.clone())
            .unwrap_or_else(|| self.selected.clone());
        let text = text.to_lowercase();
        self.content.replace_range(range.clone(), &text);
        let cursor = range.start + text.len();
        self.selected = cursor..cursor;
        self.reversed = false;
        self.marked = None;
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range
            .as_ref()
            .map(|range| self.range_from_utf16(range))
            .or_else(|| self.marked.clone())
            .unwrap_or_else(|| self.selected.clone());
        let normalized = text.to_lowercase();
        self.content.replace_range(range.clone(), &normalized);
        self.marked =
            (!normalized.is_empty()).then_some(range.start..range.start + normalized.len());
        self.selected = selected
            .as_ref()
            .map(|selected| {
                range.start + lowercase_utf16_offset(text, selected.start)
                    ..range.start + lowercase_utf16_offset(text, selected.end)
            })
            .unwrap_or_else(|| range.start + normalized.len()..range.start + normalized.len());
        self.reversed = false;
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
        let origin_x = self.last_origin_x.unwrap_or(bounds.left());
        let range = self.range_from_utf16(&range);
        Some(Bounds::from_corners(
            point(origin_x + line.x_for_index(range.start), bounds.top()),
            point(origin_x + line.x_for_index(range.end), bounds.bottom()),
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
        Some(self.offset_to_utf16(index))
    }
}

fn lowercase_utf16_offset(text: &str, offset: usize) -> usize {
    let mut remaining = offset;
    let mut normalized_offset = 0;
    for character in text.chars() {
        if remaining == 0 {
            break;
        }
        remaining = remaining.saturating_sub(character.len_utf16());
        normalized_offset += character.to_lowercase().map(char::len_utf8).sum::<usize>();
    }
    normalized_offset
}

impl Focusable for BrowserSearch {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl gpui::Render for BrowserSearch {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .debug_selector(|| "browser-search".into())
            .flex()
            .min_w_0()
            .overflow_hidden()
            .h(px(24.0))
            .px_1()
            .items_center()
            .bg(gpui::rgb(crate::theme::color(
                crate::theme::PaletteRole::Raised,
            )))
            .border_1()
            .border_color(gpui::rgb(crate::theme::color(
                crate::theme::PaletteRole::Border,
            )))
            .track_focus(&self.focus)
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .child(BrowserSearchElement { input: cx.entity() })
    }
}

pub(super) struct BrowserSearchElement {
    input: Entity<BrowserSearch>,
}
pub struct PrepaintState {
    line: Option<ShapedLine>,
    cursor: Option<PaintQuad>,
    selection: Option<PaintQuad>,
    origin_x: Pixels,
}

impl IntoElement for BrowserSearchElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for BrowserSearchElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;
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
        style.size.width = relative(1.).into();
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
    ) -> PrepaintState {
        let input = self.input.read(cx);
        let style = window.text_style();
        let (text, color) = if input.content.is_empty() {
            (
                "search · cmd+f".to_owned(),
                gpui::rgb(crate::theme::color(crate::theme::PaletteRole::Muted)).into(),
            )
        } else {
            (
                input.content.clone(),
                gpui::rgb(crate::theme::color(crate::theme::PaletteRole::Text)).into(),
            )
        };
        let run = TextRun {
            len: text.len(),
            font: style.font(),
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let runs = if let Some(marked) = input.marked.as_ref() {
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
        let font_size = style.font_size.to_pixels(window.rem_size());
        let line = window
            .text_system()
            .shape_line(text.into(), font_size, &runs, None);
        let cursor_x = line.x_for_index(input.cursor());
        let max_cursor_x = bounds.size.width - px(2.0);
        let text_shift = if cursor_x > max_cursor_x {
            cursor_x - max_cursor_x
        } else if cursor_x < px(0.0) {
            cursor_x
        } else {
            px(0.0)
        };
        let origin_x = bounds.left() - text_shift;
        let cursor = if input.selected.is_empty() {
            Some(fill(
                Bounds::new(
                    point(origin_x + cursor_x, bounds.top()),
                    size(px(1.0), bounds.bottom() - bounds.top()),
                ),
                gpui::rgb(crate::theme::color(crate::theme::PaletteRole::Secondary)),
            ))
        } else {
            None
        };
        let selection = if input.selected.is_empty() {
            None
        } else {
            Some(fill(
                Bounds::from_corners(
                    point(
                        origin_x + line.x_for_index(input.selected.start),
                        bounds.top(),
                    ),
                    point(
                        origin_x + line.x_for_index(input.selected.end),
                        bounds.bottom(),
                    ),
                ),
                rgba(0x3350a0ff),
            ))
        };
        PrepaintState {
            line: Some(line),
            cursor,
            selection,
            origin_x,
        }
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        state: &mut PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus = self.input.read(cx).focus.clone();
        window.handle_input(
            &focus,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        if let Some(selection) = state.selection.take() {
            window.paint_quad(selection);
        }
        if let Some(line) = state.line.take() {
            let _ = line.paint(
                point(state.origin_x, bounds.top()),
                window.line_height(),
                window,
                cx,
            );
            if focus.is_focused(window)
                && let Some(cursor) = state.cursor.take()
            {
                window.paint_quad(cursor);
            }
            self.input.update(cx, |input, _| {
                input.last_layout = Some(line);
                input.last_bounds = Some(bounds);
                input.last_origin_x = Some(state.origin_x);
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::lowercase_utf16_offset;

    #[test]
    fn unicode_case_expansion_keeps_ime_offsets_in_normalized_query() {
        let raw = "İA";
        let normalized = raw.to_lowercase();
        assert_eq!(normalized, "i\u{307}a");
        assert_eq!(lowercase_utf16_offset(raw, 1), "i\u{307}".len());
        assert_eq!(lowercase_utf16_offset(raw, 2), normalized.len());
    }

    #[test]
    fn supplementary_unicode_offsets_follow_utf16_selection_units() {
        let raw = "🎛A";
        assert_eq!(lowercase_utf16_offset(raw, 1), "🎛".len());
        assert_eq!(lowercase_utf16_offset(raw, 2), "🎛".len());
        assert_eq!(lowercase_utf16_offset(raw, 3), "🎛a".len());
    }
}
