//! Native single-line text entry with pointer selection, UTF-16 IME and clipboard.
//! Search mode normalizes typed queries through the supplied text case; value mode
//! preserves source spelling while rendering through that case. Callers own domain
//! validation, canonical commits and external content. Only search reserves a clear
//! action slot; its accessibility receipt cannot edit a newer composition.
//! Pointer selection uses the shared Window lease, independently of content or
//! canonical edit acknowledgements. Input loss clears only the held anchor;
//! newer text/IME drafts and the last valid selection remain owned by this field.
use crate::{ActionStyle, ControlStyle, TextCase, TextClickSelection, action};
use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, Element, ElementId, ElementInputHandler,
    Entity, EntityInputHandler, FocusHandle, Focusable, GlobalElementId, LayoutId, MouseButton,
    MouseDownEvent, PaintQuad, Pixels, Point, ShapedLine, Style, TextRun, UTF16Selection,
    UnderlineStyle, Window, div, fill, point, prelude::*, px, relative, size,
};
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

/// Search queries and case-preserving source values share one input implementation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextInputKind {
    Search,
    Value,
}
/// Caller-supplied typography, labels and bounds. The native editor does not own
/// names, filters, project validation or persistence. Styles come from the owner.
#[derive(Clone)]
pub struct TextInputConfig {
    pub id: gpui::SharedString,
    pub kind: TextInputKind,
    pub style: ControlStyle,
    pub action_style: ActionStyle,
    pub font_family: gpui::SharedString,
    pub placeholder: gpui::SharedString,
    pub clear_name: gpui::SharedString,
    pub clear_glyph: gpui::SharedString,
    /// Maximum resulting bytes admitted from native typing/paste/composition.
    /// External state still belongs to its validating owner.
    pub max_bytes: usize,
}
/// Native text/composition edits and explicit clear notify the owner. Caret-only
/// changes and external set_text only repaint; this is not a canonical commit.
#[derive(Clone, Copy, Debug)]
pub struct TextInputChanged;
impl gpui::EventEmitter<TextInputChanged> for TextInput {}
/// Install text editing once; the containing app retains Enter/Escape/navigation.
pub fn bind_text_input_keys(cx: &mut App) {
    cx.bind_keys([
        gpui::KeyBinding::new("backspace", TextBackspace, Some("hex_text_input")),
        gpui::KeyBinding::new("delete", TextDelete, Some("hex_text_input")),
        gpui::KeyBinding::new("left", TextLeft, Some("hex_text_input")),
        gpui::KeyBinding::new("right", TextRight, Some("hex_text_input")),
        gpui::KeyBinding::new("shift-left", TextSelectLeft, Some("hex_text_input")),
        gpui::KeyBinding::new("shift-right", TextSelectRight, Some("hex_text_input")),
        gpui::KeyBinding::new("cmd-a", TextSelectAll, Some("hex_text_input")),
        gpui::KeyBinding::new("home", TextHome, Some("hex_text_input")),
        gpui::KeyBinding::new("cmd-left", TextHome, Some("hex_text_input")),
        gpui::KeyBinding::new("end", TextEnd, Some("hex_text_input")),
        gpui::KeyBinding::new("cmd-right", TextEnd, Some("hex_text_input")),
        gpui::KeyBinding::new("cmd-v", TextPaste, Some("hex_text_input")),
        gpui::KeyBinding::new("cmd-c", TextCopy, Some("hex_text_input")),
        gpui::KeyBinding::new("cmd-x", TextCut, Some("hex_text_input")),
    ]);
}
#[derive(Clone, Copy)]
struct MouseSelection {
    anchor: usize,
    origin: Point<Pixels>,
    dragging: bool,
}

gpui::actions!(
    hex_text_input,
    [
        TextBackspace,
        TextDelete,
        TextLeft,
        TextRight,
        TextSelectLeft,
        TextSelectRight,
        TextSelectAll,
        TextHome,
        TextEnd,
        TextPaste,
        TextCopy,
        TextCut,
    ]
);

pub struct TextInput {
    content: String,
    caret_activity: u64,
    config: TextInputConfig,
    enabled: bool,
    tab_navigation: bool,
    preserve_source: bool,
    focus: FocusHandle,
    selected: Range<usize>,
    reversed: bool,
    marked: Option<Range<usize>>,
    mouse_selection: Option<MouseSelection>,
    text_click: TextClickSelection,
    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    last_origin_x: Option<Pixels>,
    // Display-only semantic receipt; filtering still reads `content` directly.
    // Revisions distinguish native button actions from an older text/composition.
    clear_accessibility_lifetime: gpui::AccessibilityLifetime,
    clear_accessibility_revision: Option<u64>,
    clear_accessibility_source: (String, Option<Range<usize>>),
}

impl TextInput {
    pub fn new(cx: &mut Context<Self>, config: TextInputConfig) -> Self {
        Self {
            content: String::new(),
            caret_activity: 0,
            preserve_source: config.kind == TextInputKind::Value,
            config,
            enabled: true,
            tab_navigation: false,
            focus: cx.focus_handle(),
            selected: 0..0,
            reversed: false,
            marked: None,
            mouse_selection: None,
            text_click: TextClickSelection::default(),
            last_layout: None,
            last_bounds: None,
            last_origin_x: None,
            clear_accessibility_lifetime: gpui::AccessibilityLifetime::default(),
            clear_accessibility_revision: Some(0),
            clear_accessibility_source: (String::new(), None),
        }
    }

    pub fn text(&self) -> &str {
        &self.content
    }
    /// Installs caller-validated external state, preserving its bytes and casing.
    /// Repaints without emitting a native edit notification.
    pub fn set_text(&mut self, content: &str, cx: &mut Context<Self>) {
        self.mouse_selection = None;
        self.text_click.clear();
        self.marked = None;
        self.content = content.to_owned();
        self.selected = self.content.len()..self.content.len();
        self.reversed = false;
        cx.notify();
    }
    pub fn select_all_text(&mut self, cx: &mut Context<Self>) {
        self.selected = 0..self.content.len();
        self.reversed = false;
        cx.notify();
    }
    pub fn set_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.enabled != enabled {
            self.enabled = enabled;
            self.focus = self
                .focus
                .clone()
                .tab_index(0)
                .tab_stop(self.tab_navigation && enabled);
            self.mouse_selection = None;
            self.marked = None;
            cx.notify();
        }
    }
    pub fn set_tab_navigation(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.tab_navigation = enabled;
        self.focus = self
            .focus
            .clone()
            .tab_index(0)
            .tab_stop(enabled && self.enabled);
        cx.notify();
    }
    pub fn set_external_style(
        &mut self,
        style: ControlStyle,
        action_style: ActionStyle,
        cx: &mut Context<Self>,
    ) -> Result<(), &'static str> {
        let style = style.validate()?;
        if self.config.style != style || self.config.action_style != action_style {
            self.config.style = style;
            self.config.action_style = action_style;
            cx.notify();
        }
        Ok(())
    }
    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }
    pub fn focus(&self, window: &mut Window) {
        if self.enabled {
            self.focus.focus(window);
        }
    }
    pub fn focus_and_select_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.mouse_selection = None;
        self.text_click.clear();
        self.focus(window);
        self.unmark_text(window, cx);
        self.select_all(&TextSelectAll, window, cx);
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.mouse_selection = None;
        self.text_click.clear();
        self.content.clear();
        self.selected = 0..0;
        self.reversed = false;
        self.marked = None;
        cx.emit(TextInputChanged);
        cx.notify();
    }

    fn has_committed_search_text(&self) -> bool {
        self.enabled
            && !self.preserve_source
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
        let active = self.enabled && self.has_committed_search_text();
        div()
            .debug_selector({
                let id = format!("{}-clear-slot", self.config.id);
                move || id.clone()
            })
            .w(px(20.0))
            .h(px(20.0))
            .flex_shrink_0()
            .children(active.then(|| {
                let button = action(self.config.clear_glyph.clone(), self.config.action_style)
                    .id(gpui::SharedString::from(format!(
                        "{}-clear",
                        self.config.id
                    )))
                    .debug_selector({
                        let id = format!("{}-clear", self.config.id);
                        move || id.clone()
                    })
                    .size(px(20.0))
                    .tooltip({
                        let label = self.config.clear_name.clone();
                        let style = self.config.style;
                        let font = self.config.font_family.clone();
                        move |_, cx| {
                            cx.new(|_| InputTooltip {
                                label: label.clone(),
                                style,
                                font: font.clone(),
                            })
                            .into()
                        }
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
                        name: self.config.clear_name.clone(),
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

    /// Most recent shaped display text; source spelling remains available via text.
    pub fn painted_text(&self) -> Option<&str> {
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
        self.caret_activity = self.caret_activity.wrapping_add(1);
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
        self.caret_activity = self.caret_activity.wrapping_add(1);
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

    fn backspace(&mut self, _: &TextBackspace, window: &mut Window, cx: &mut Context<Self>) {
        if !self.enabled {
            return;
        }
        if self.selected.is_empty() {
            let start = self.previous_boundary(self.cursor());
            self.selected = start..self.cursor();
        }
        self.replace("", window, cx);
    }

    fn delete(&mut self, _: &TextDelete, window: &mut Window, cx: &mut Context<Self>) {
        if !self.enabled {
            return;
        }
        if self.selected.is_empty() {
            let end = self.next_boundary(self.cursor());
            self.selected = self.cursor()..end;
        }
        self.replace("", window, cx);
    }

    fn left(&mut self, _: &TextLeft, _: &mut Window, cx: &mut Context<Self>) {
        if !self.enabled {
            return;
        }
        let target = if self.selected.is_empty() {
            self.previous_boundary(self.cursor())
        } else {
            self.selected.start
        };
        self.move_to(target, cx);
    }

    fn right(&mut self, _: &TextRight, _: &mut Window, cx: &mut Context<Self>) {
        if !self.enabled {
            return;
        }
        let target = if self.selected.is_empty() {
            self.next_boundary(self.cursor())
        } else {
            self.selected.end
        };
        self.move_to(target, cx);
    }

    fn select_left(&mut self, _: &TextSelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        if !self.enabled {
            return;
        }
        self.select_to(self.previous_boundary(self.cursor()), cx);
    }

    fn select_right(&mut self, _: &TextSelectRight, _: &mut Window, cx: &mut Context<Self>) {
        if !self.enabled {
            return;
        }
        self.select_to(self.next_boundary(self.cursor()), cx);
    }

    fn select_all(&mut self, _: &TextSelectAll, _: &mut Window, cx: &mut Context<Self>) {
        if !self.enabled {
            return;
        }
        self.mouse_selection = None;
        self.text_click.clear();
        self.selected = 0..self.content.len();
        self.reversed = false;
        cx.notify();
    }

    fn home(&mut self, _: &TextHome, _: &mut Window, cx: &mut Context<Self>) {
        if !self.enabled {
            return;
        }
        self.move_to(0, cx);
    }

    fn end(&mut self, _: &TextEnd, _: &mut Window, cx: &mut Context<Self>) {
        if !self.enabled {
            return;
        }
        self.move_to(self.content.len(), cx);
    }

    fn paste(&mut self, _: &TextPaste, window: &mut Window, cx: &mut Context<Self>) {
        if !self.enabled {
            return;
        }
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            let clean = text.replace(['\n', '\r'], " ");
            self.replace(&clean, window, cx);
        }
    }

    fn copy(&mut self, _: &TextCopy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.enabled {
            return;
        }
        if let Some(text) = self.content.get(self.selected.clone())
            && !text.is_empty()
        {
            cx.write_to_clipboard(ClipboardItem::new_string(text.to_owned()));
        }
    }

    fn cut(&mut self, _: &TextCut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.enabled {
            return;
        }
        if let Some(text) = self.content.get(self.selected.clone())
            && !text.is_empty()
        {
            cx.write_to_clipboard(ClipboardItem::new_string(text.to_owned()));
            self.replace("", window, cx);
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
        let index = self
            .config
            .style
            .text_case
            .source_offset(&self.content, index);
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
        if !self.enabled {
            return;
        }
        let owner = cx.entity().downgrade();
        let cancelled = owner.clone();
        if !window.capture_pointer(
            cx.entity_id(),
            MouseButton::Left,
            CursorStyle::IBeam,
            move |event, window, cx| {
                let _ = owner.update(cx, |input, cx| {
                    if let Some(event) = event.downcast_ref::<gpui::MouseMoveEvent>() {
                        input.mouse_move(event, window, cx);
                    } else if let Some(event) = event.downcast_ref::<gpui::MouseUpEvent>()
                        && event.button == MouseButton::Left
                        && input.mouse_selection.take().is_some()
                    {
                        cx.notify();
                    }
                });
            },
            move |_, cx| {
                let _ = cancelled.update(cx, |input, cx| input.cancel_mouse_selection(cx));
            },
            cx,
        ) {
            return;
        }
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
        if !self.preserve_source && !self.focus.is_focused(window) {
            self.focus_and_select_all(window, cx);
        } else if let Some(all) = toggle {
            if all {
                self.select_all(&TextSelectAll, window, cx);
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
        // A stationary press still needs a held witness in cached paint.
        cx.notify();
        // The field owns this click; its browser parent owns non-text clicks.
        cx.stop_propagation();
    }

    fn cancel_mouse_selection(&mut self, cx: &mut Context<Self>) {
        if self.mouse_selection.take().is_some() {
            self.text_click.clear();
            cx.notify();
        }
    }

    fn mouse_move(
        &mut self,
        event: &gpui::MouseMoveEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        if !self.enabled {
            return;
        }
        let Some(selection) = self.mouse_selection else {
            return;
        };
        if event.pressed_button != Some(MouseButton::Left) || !self.focus.is_focused(window) {
            self.cancel_mouse_selection(cx);
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

impl EntityInputHandler for TextInput {
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

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.caret_activity = self.caret_activity.wrapping_add(1);
        cx.notify();
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
        if !self.enabled {
            return;
        }
        self.caret_activity = self.caret_activity.wrapping_add(1);
        cx.notify();
        if text.len() > self.config.max_bytes {
            return;
        }
        if range.as_ref().is_some_and(|r| r.start > r.end) {
            return;
        }
        self.mouse_selection = None;
        self.text_click.clear();
        let range = range
            .as_ref()
            .map(|range| self.range_from_utf16(range))
            .or_else(|| self.marked.clone())
            .unwrap_or_else(|| self.selected.clone());
        let text = if self.preserve_source {
            text.replace(['\n', '\r'], " ")
        } else {
            self.config.style.text_case.present(text)
        };
        if self.content.len() - range.len() + text.len() > self.config.max_bytes
            || (self.preserve_source && text.chars().any(char::is_control))
        {
            return;
        }
        self.content.replace_range(range.clone(), &text);
        let cursor = range.start + text.len();
        self.selected = cursor..cursor;
        self.reversed = false;
        self.marked = None;
        cx.emit(TextInputChanged);
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
        if !self.enabled {
            return;
        }
        self.caret_activity = self.caret_activity.wrapping_add(1);
        cx.notify();
        if text.len() > self.config.max_bytes {
            return;
        }
        if range.as_ref().is_some_and(|r| r.start > r.end) {
            return;
        }
        if selected.as_ref().is_some_and(|r| r.start > r.end) {
            return;
        }
        self.mouse_selection = None;
        self.text_click.clear();
        let range = range
            .as_ref()
            .map(|range| self.range_from_utf16(range))
            .or_else(|| self.marked.clone())
            .unwrap_or_else(|| self.selected.clone());
        let normalized = if self.preserve_source {
            text.to_owned()
        } else {
            self.config.style.text_case.present(text)
        };
        if self.content.len() - range.len() + normalized.len() > self.config.max_bytes
            || (self.preserve_source && normalized.chars().any(char::is_control))
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
                    + if self.preserve_source || self.config.style.text_case == TextCase::Preserve {
                        utf16_offset(text, selected.start)
                    } else {
                        lowercase_utf16_offset(text, selected.start)
                    }
                    ..range.start
                        + if self.preserve_source
                            || self.config.style.text_case == TextCase::Preserve
                        {
                            utf16_offset(text, selected.end)
                        } else {
                            lowercase_utf16_offset(text, selected.end)
                        }
            })
            .unwrap_or_else(|| range.start + normalized.len()..range.start + normalized.len());
        self.reversed = false;
        cx.emit(TextInputChanged);
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
                        self.config
                            .style
                            .text_case
                            .display_offset(&self.content, range.start),
                    ),
                bounds.top(),
            ),
            point(
                origin_x
                    + line.x_for_index(
                        self.config
                            .style
                            .text_case
                            .display_offset(&self.content, range.end),
                    ),
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
        Some(
            self.offset_to_utf16(
                self.config
                    .style
                    .text_case
                    .source_offset(&self.content, index),
            ),
        )
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

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl gpui::Render for TextInput {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let input = div()
            .debug_selector({
                let id = self.config.id.clone();
                move || id.to_string()
            })
            .flex()
            .min_w_0()
            .overflow_hidden()
            .h(px(if self.preserve_source { 16.0 } else { 24.0 }))
            .px_1()
            .items_center()
            .bg(gpui::rgb(self.config.style.raised))
            .font_family(self.config.font_family.clone())
            .text_size(px(self.config.style.font_size))
            .opacity(if self.enabled { 1.0 } else { 0.4 })
            .key_context("hex_text_input")
            .border_1()
            .border_color(gpui::rgb(self.config.style.border))
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
        if self.preserve_source {
            input.child(TextInputElement { input: cx.entity() })
        } else {
            input
                .child(
                    div()
                        .debug_selector({
                            let id = format!("{}-text", self.config.id);
                            move || id.clone()
                        })
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .child(TextInputElement { input: cx.entity() }),
                )
                .child(self.clear_search_slot(window, cx))
        }
    }
}

pub struct TextInputElement {
    input: Entity<TextInput>,
}
pub struct PrepaintState {
    line: Option<ShapedLine>,
    cursor: Option<PaintQuad>,
    selection: Option<PaintQuad>,
    origin_x: Pixels,
}

impl IntoElement for TextInputElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for TextInputElement {
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
                if input.preserve_source {
                    String::new()
                } else {
                    input.config.placeholder.to_string()
                },
                gpui::rgb(input.config.style.muted).into(),
            )
        } else {
            (
                input.config.style.text_case.present(&input.content),
                gpui::rgb(input.config.style.text).into(),
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
            input
                .config
                .style
                .text_case
                .display_offset(&input.content, range.start)
                ..input
                    .config
                    .style
                    .text_case
                    .display_offset(&input.content, range.end)
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
        let font_size = style.font_size.to_pixels(window.rem_size());
        let line = window
            .text_system()
            .shape_line(text.into(), font_size, &runs, None);
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
                .config
                .style
                .text_case
                .display_offset(&input.content, input.cursor()),
        );
        let max_cursor_x = (bounds.size.width - cell_width).max(px(0.0));
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
                    size(
                        cell_width.min(bounds.size.width),
                        bounds.bottom() - bounds.top(),
                    ),
                ),
                gpui::rgb(input.config.style.accent),
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
                                input
                                    .config
                                    .style
                                    .text_case
                                    .display_offset(&input.content, input.selected.start),
                            ),
                        bounds.top(),
                    ),
                    point(
                        origin_x
                            + line.x_for_index(
                                input
                                    .config
                                    .style
                                    .text_case
                                    .display_offset(&input.content, input.selected.end),
                            ),
                        bounds.bottom(),
                    ),
                ),
                gpui::rgb(input.config.style.selection),
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
        let input = self.input.read(cx);
        if input.mouse_selection.is_some() && input.enabled && input.focus.is_focused(window) {
            window.renew_pointer_capture(self.input.entity_id(), CursorStyle::IBeam);
        }
        let focus = input.focus.clone();
        if input.enabled {
            window.handle_input(
                &focus,
                ElementInputHandler::new(bounds, self.input.clone()),
                cx,
            );
        }
        if focus.is_focused(window)
            && let Some(selection) = state.selection.take()
        {
            window.paint_quad(selection);
        }
        if focus.is_focused(window)
            && self.input.read(cx).enabled
            && let Some(cursor) = state.cursor.take()
        {
            let activity = self.input.read(cx).caret_activity;
            crate::text_caret::paint(cursor, self.input.entity_id(), activity, window, cx);
        }
        if let Some(line) = state.line.take() {
            let _ = line.paint(
                point(state.origin_x, bounds.top()),
                window.line_height(),
                window,
                cx,
            );
            self.input.update(cx, |input, _| {
                input.last_layout = Some(line);
                input.last_bounds = Some(bounds);
                input.last_origin_x = Some(state.origin_x);
            });
        }
    }
}

struct InputTooltip {
    label: gpui::SharedString,
    style: ControlStyle,
    font: gpui::SharedString,
}
impl gpui::Render for InputTooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .bg(gpui::rgb(self.style.raised))
            .border_1()
            .border_color(gpui::rgb(self.style.border))
            .font_family(self.font.clone())
            .text_size(px(self.style.font_size))
            .text_color(gpui::rgb(self.style.text))
            .child(self.label.clone())
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

#[cfg(test)]
#[path = "text_input/name_case_tests.rs"]
mod name_case_tests;
