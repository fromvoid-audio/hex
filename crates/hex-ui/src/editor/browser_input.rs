//! Native GPUI text input used by the browser filter, including IME and clipboard.
//! Only query mode reserves a clear-action slot. Pointer and accessibility actions
//! clear/refocus this same input; semantic receipts never own filtering or history.
use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, Element, ElementId, ElementInputHandler,
    Entity, EntityInputHandler, FocusHandle, Focusable, GlobalElementId, LayoutId, MouseButton,
    MouseDownEvent, PaintQuad, Pixels, Point, ShapedLine, Style, TextRun, UTF16Selection,
    UnderlineStyle, Window, div, fill, point, prelude::*, px, relative, rgba, size,
};
use hex_widgets::TextCase;
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

#[derive(Clone, Copy)]
struct MouseSelection {
    anchor: usize,
    origin: Point<Pixels>,
    dragging: bool,
}

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
    clip_name: bool,
    focus: FocusHandle,
    selected: Range<usize>,
    reversed: bool,
    marked: Option<Range<usize>>,
    mouse_selection: Option<MouseSelection>,
    text_click: hex_widgets::TextClickSelection,
    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    last_origin_x: Option<Pixels>,
    // Display-only semantic receipt; filtering still reads `content` directly.
    // Revisions distinguish native button actions from an older text/composition.
    clear_accessibility_lifetime: gpui::AccessibilityLifetime,
    clear_accessibility_revision: Option<u64>,
    clear_accessibility_source: (String, Option<Range<usize>>),
}

impl BrowserSearch {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            content: String::new(),
            clip_name: false,
            focus: cx.focus_handle(),
            selected: 0..0,
            reversed: false,
            marked: None,
            mouse_selection: None,
            text_click: hex_widgets::TextClickSelection::default(),
            last_layout: None,
            last_bounds: None,
            last_origin_x: None,
            clear_accessibility_lifetime: gpui::AccessibilityLifetime::default(),
            clear_accessibility_revision: Some(0),
            clear_accessibility_source: (String::new(), None),
        }
    }

    pub(super) fn for_clip_name(content: String, cx: &mut Context<Self>) -> Self {
        let mut input = Self::new(cx);
        input.selected = 0..content.len();
        input.content = content;
        input.clip_name = true;
        input
    }

    pub(super) fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }
    pub(super) fn focus(&self, window: &mut Window) {
        self.focus.focus(window);
    }
    pub(super) fn focus_and_select_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.mouse_selection = None;
        self.text_click.clear();
        self.focus(window);
        self.unmark_text(window, cx);
        self.select_all(&BrowserSelectAll, window, cx);
    }

    pub(super) fn clear(&mut self, cx: &mut Context<Self>) {
        self.mouse_selection = None;
        self.text_click.clear();
        self.content.clear();
        self.selected = 0..0;
        self.reversed = false;
        self.marked = None;
        cx.notify();
    }

    fn has_committed_search_text(&self) -> bool {
        !self.clip_name
            && !self.content.is_empty()
            && self
                .marked
                .as_ref()
                .is_none_or(|marked| marked.start > 0 || marked.end < self.content.len())
    }

    fn clear_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.has_committed_search_text() {
            self.clear(cx);
            self.focus(window);
        }
    }

    fn clear_search_slot(&mut self, window: &Window, cx: &mut Context<Self>) -> gpui::Div {
        if self.clear_accessibility_source.0 != self.content
            || self.clear_accessibility_source.1 != self.marked
        {
            // Exhaustion retires native actions rather than reusing a receipt;
            // ordinary pointer/text input remains available on the actual owner.
            self.clear_accessibility_revision = self
                .clear_accessibility_revision
                .and_then(|revision| revision.checked_add(1));
            self.clear_accessibility_source = (self.content.clone(), self.marked.clone());
        }
        let active = self.has_committed_search_text();
        div()
            .debug_selector(|| "browser-search-clear-slot".into())
            .w(px(20.0))
            .h(px(20.0))
            .flex_shrink_0()
            .children(active.then(|| {
                let button =
                    hex_widgets::action(crate::theme::text("×"), crate::theme::action_style())
                        .id("browser-search-clear")
                        .debug_selector(|| "browser-search-clear".into())
                        .size(px(20.0))
                        .tooltip(|_, cx| {
                            cx.new(|_| super::transport::MiniTooltip {
                                description: crate::theme::text("clear search"),
                            })
                            .into()
                        })
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|input, _, window, cx| {
                                cx.stop_propagation();
                                input.clear_search(window, cx);
                            }),
                        );
                let Some(revision) = self.clear_accessibility_revision else {
                    return button;
                };
                let entity = cx.entity();
                let owner = entity.downgrade();
                let source = self.clear_accessibility_source.clone();
                button.accessibility(
                    gpui::AccessibilityDescriptor {
                        id: gpui::AccessibilityId(entity.entity_id().as_u64()),
                        revision,
                        lifetime: self.clear_accessibility_lifetime.weak(),
                        name: crate::theme::text("clear search"),
                        value: gpui::AccessibilityValue::Button,
                        enabled: true,
                        focused: self.focus.is_focused(window),
                        status: None,
                    },
                    move |request, window, cx| {
                        owner
                            .update(cx, |input, cx| {
                                if input.clear_accessibility_revision != Some(request.revision)
                                    || (input.content.as_str(), &input.marked)
                                        != (source.0.as_str(), &source.1)
                                    || !input.has_committed_search_text()
                                {
                                    return false;
                                }
                                match request.action {
                                    gpui::AccessibilityAction::Focus => input.focus(window),
                                    gpui::AccessibilityAction::Press => {
                                        input.clear_search(window, cx)
                                    }
                                    _ => return false,
                                }
                                true
                            })
                            .unwrap_or(false)
                    },
                )
            }))
    }

    /// Browser-owned location restoration, without focus or a text edit event.
    pub(super) fn restore_query(&mut self, content: &str, cx: &mut Context<Self>) {
        // Reuse clear's complete input/gesture invariants (including any shared
        // selection policy), then publish only the final value at update end.
        self.clear(cx);
        self.content.push_str(content);
        self.selected = self.content.len()..self.content.len();
    }

    #[cfg(all(test, feature = "ui-test-support"))]
    pub(super) fn painted_text_for_test(&self) -> Option<&str> {
        self.last_layout.as_ref().map(|line| line.text.as_ref())
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
            .grapheme_indices(true)
            .next_back()
            .map_or(0, |(index, _)| index)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        let offset = offset.min(self.content.len());
        self.content
            .grapheme_indices(true)
            .find(|(index, _)| *index > offset)
            .map_or(self.content.len(), |(index, _)| index)
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.mouse_selection = None;
        self.text_click.clear();
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
        self.mouse_selection = None;
        self.text_click.clear();
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
        self.mouse_selection = None;
        self.text_click.clear();
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
        let index = line.closest_index_for_x(point.x - self.last_origin_x.unwrap_or(bounds.left()));
        let index = TextCase::Lowercase.source_offset(&self.content, index);
        if index == self.content.len() {
            return index;
        }
        self.content
            .grapheme_indices(true)
            .take_while(|(boundary, _)| *boundary <= index)
            .last()
            .map_or(0, |(boundary, _)| boundary)
    }

    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let focused = self.focus.is_focused(window);
        let mut gesture = self.text_click;
        if !focused || event.modifiers.shift {
            gesture.clear();
        }
        let toggle = (!event.modifiers.shift)
            .then(|| {
                gesture.selection_for_down(
                    event.click_count,
                    focused && !self.content.is_empty() && self.selected == (0..self.content.len()),
                )
            })
            .flatten();
        let offset = self.index_for_point(event.position);
        let anchor = if event.modifiers.shift && self.focus.is_focused(window) {
            if self.reversed {
                self.selected.end
            } else {
                self.selected.start
            }
        } else {
            offset
        };
        self.unmark_text(window, cx);
        if !self.clip_name && !self.focus.is_focused(window) {
            self.focus_and_select_all(window, cx);
        } else if let Some(all) = toggle {
            if all {
                self.select_all(&BrowserSelectAll, window, cx);
            } else {
                self.move_to(offset, cx);
            }
        } else if event.modifiers.shift {
            self.select_to(offset, cx);
        } else {
            self.move_to(offset, cx);
        }
        self.text_click = gesture;
        self.focus.focus(window);
        self.mouse_selection = Some(MouseSelection {
            anchor,
            origin: event.position,
            dragging: false,
        });
        // The field owns this click; its browser parent owns non-text clicks.
        cx.stop_propagation();
    }

    fn mouse_move(
        &mut self,
        event: &gpui::MouseMoveEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let Some(selection) = self.mouse_selection else {
            return;
        };
        if event.pressed_button != Some(MouseButton::Left) || !self.focus.is_focused(window) {
            self.mouse_selection = None;
            self.text_click.clear();
            return;
        }
        // A focus click stays Select All through ordinary pointer jitter. A
        // deliberate held drag selects from its down-point, even outside bounds.
        if !selection.dragging
            && f32::from(event.position.x - selection.origin.x).abs() < 2.0
            && f32::from(event.position.y - selection.origin.y).abs() < 2.0
        {
            return;
        }
        self.text_click.clear();
        if let Some(selection) = self.mouse_selection.as_mut() {
            selection.dragging = true;
        }
        let offset = self.index_for_point(event.position);
        let selected = selection.anchor.min(offset)..selection.anchor.max(offset);
        let reversed = offset < selection.anchor;
        if self.selected != selected || self.reversed != reversed {
            self.selected = selected;
            self.reversed = reversed;
            cx.notify();
        }
        cx.stop_propagation();
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
        self.text_click.clear();
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.mouse_selection = None;
        self.text_click.clear();
        let range = range
            .as_ref()
            .map(|range| self.range_from_utf16(range))
            .or_else(|| self.marked.clone())
            .unwrap_or_else(|| self.selected.clone());
        let text = if self.clip_name {
            text.replace(['\n', '\r'], " ")
        } else {
            text.to_lowercase()
        };
        if self.clip_name
            && (self.content.len() - range.len() + text.len() > hex_project::MAX_NAME_BYTES
                || text.chars().any(char::is_control))
        {
            return;
        }
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
        self.mouse_selection = None;
        self.text_click.clear();
        let range = range
            .as_ref()
            .map(|range| self.range_from_utf16(range))
            .or_else(|| self.marked.clone())
            .unwrap_or_else(|| self.selected.clone());
        let normalized = if self.clip_name {
            text.to_owned()
        } else {
            text.to_lowercase()
        };
        if self.clip_name
            && (self.content.len() - range.len() + normalized.len() > hex_project::MAX_NAME_BYTES
                || normalized.chars().any(char::is_control))
        {
            return;
        }
        self.content.replace_range(range.clone(), &normalized);
        self.marked =
            (!normalized.is_empty()).then_some(range.start..range.start + normalized.len());
        self.selected = selected
            .as_ref()
            .map(|selected| {
                range.start
                    + if self.clip_name {
                        utf16_offset(text, selected.start)
                    } else {
                        lowercase_utf16_offset(text, selected.start)
                    }
                    ..range.start
                        + if self.clip_name {
                            utf16_offset(text, selected.end)
                        } else {
                            lowercase_utf16_offset(text, selected.end)
                        }
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
            point(
                origin_x
                    + line.x_for_index(
                        TextCase::Lowercase.display_offset(&self.content, range.start),
                    ),
                bounds.top(),
            ),
            point(
                origin_x
                    + line
                        .x_for_index(TextCase::Lowercase.display_offset(&self.content, range.end)),
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
        Some(self.offset_to_utf16(TextCase::Lowercase.source_offset(&self.content, index)))
    }
}

fn utf16_offset(text: &str, offset: usize) -> usize {
    let mut remaining = offset;
    text.chars()
        .take_while(|ch| {
            if remaining == 0 {
                false
            } else {
                remaining = remaining.saturating_sub(ch.len_utf16());
                true
            }
        })
        .map(char::len_utf8)
        .sum()
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let input = div()
            .debug_selector({
                let name = self.clip_name;
                move || {
                    if name {
                        "clip-name-input".into()
                    } else {
                        "browser-search".into()
                    }
                }
            })
            .flex()
            .min_w_0()
            .overflow_hidden()
            .h(px(if self.clip_name { 16.0 } else { 24.0 }))
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
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down));
        if self.clip_name {
            input.child(BrowserSearchElement { input: cx.entity() })
        } else {
            input
                .child(
                    div()
                        .debug_selector(|| "browser-search-text".into())
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .child(BrowserSearchElement { input: cx.entity() }),
                )
                .child(self.clear_search_slot(window, cx))
        }
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
                if input.clip_name {
                    String::new()
                } else {
                    "search · cmd+f".to_owned()
                },
                gpui::rgb(crate::theme::color(crate::theme::PaletteRole::Muted)).into(),
            )
        } else {
            (
                TextCase::Lowercase.present(&input.content),
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
        let marked = input.marked.as_ref().map(|range| {
            TextCase::Lowercase.display_offset(&input.content, range.start)
                ..TextCase::Lowercase.display_offset(&input.content, range.end)
        });
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
        let font_size = style.font_size.to_pixels(window.rem_size());
        let line = window
            .text_system()
            .shape_line(text.into(), font_size, &runs, None);
        let cursor_x =
            line.x_for_index(TextCase::Lowercase.display_offset(&input.content, input.cursor()));
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
                        origin_x
                            + line.x_for_index(
                                TextCase::Lowercase
                                    .display_offset(&input.content, input.selected.start),
                            ),
                        bounds.top(),
                    ),
                    point(
                        origin_x
                            + line.x_for_index(
                                TextCase::Lowercase
                                    .display_offset(&input.content, input.selected.end),
                            ),
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
        let pointer = self.input.clone();
        window.on_mouse_event(move |event: &gpui::MouseMoveEvent, phase, window, cx| {
            if phase.bubble() {
                pointer.update(cx, |input, cx| input.mouse_move(event, window, cx));
            }
        });
        let release = self.input.clone();
        window.on_mouse_event(move |event: &gpui::MouseUpEvent, phase, window, cx| {
            if phase.bubble() && event.button == MouseButton::Left {
                let owned = release.update(cx, |input, _| {
                    input.mouse_selection.take().is_some() && input.focus.is_focused(window)
                });
                if owned {
                    cx.stop_propagation();
                }
            }
        });
        let focus = self.input.read(cx).focus.clone();
        window.handle_input(
            &focus,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        if focus.is_focused(window)
            && let Some(selection) = state.selection.take()
        {
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

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "name_case_ui_test.rs"]
mod name_case_ui_test;
