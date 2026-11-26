//! Inline text drafting, cursor/selection motion, clipboard, and commit/cancel for `NumericControl`.
use super::*;

impl NumericControl {
    pub(super) fn text_index_for_point(&self, at: Point<Pixels>) -> usize {
        let (Some(bounds), Some(line)) = (self.last_bounds, self.last_layout.as_ref()) else {
            return 0;
        };
        if at.y < bounds.top() {
            return 0;
        }
        if at.y > bounds.bottom() {
            return self.text.len();
        }
        let shaped = line.closest_index_for_x(at.x - self.last_origin_x.unwrap_or(bounds.left()));
        self.value_text_case().source_offset(&self.text, shaped)
    }

    pub(super) fn enter(&mut self, _: &NumericEnter, _: &mut Window, cx: &mut Context<Self>) {
        self.text_mouse = None;
        self.text_click.clear();
        cx.stop_propagation();
        if !self.editing.is_editing() {
            return;
        }
        if self.deferred_text {
            if self.marked.is_some() {
                return;
            } else if self.text_modified {
                self.request_deferred_text(cx);
            } else {
                self.editing.0.set(false);
                self.marked = None;
                cx.notify();
            }
            return;
        }
        match self.spec.parse_with_current(&self.text, Some(self.value)) {
            Ok(value) => {
                let old = self.value;
                if old != value {
                    cx.emit(NumericControlEvent {
                        phase: NumericEventPhase::Begin,
                        value: old,
                        gesture: None,
                    });
                    self.value = value;
                    self.draft = value;
                    cx.emit(NumericControlEvent {
                        phase: NumericEventPhase::Change,
                        value,
                        gesture: None,
                    });
                    cx.emit(NumericControlEvent {
                        phase: NumericEventPhase::Commit,
                        value,
                        gesture: None,
                    });
                } else if self.commit_unchanged_text && self.text_modified {
                    cx.emit(NumericControlEvent {
                        phase: NumericEventPhase::Commit,
                        value,
                        gesture: None,
                    });
                }
                self.text = self.spec.format(value).unwrap_or_default();
                self.editing.0.set(false);
                self.invalid_text = false;
                self.marked = None;
                cx.notify();
            }
            Err(_) => {
                self.invalid_text = true;
                cx.notify();
            }
        }
    }

    /// Commit a typed value or finish an active pointer gesture before its
    /// owner hides or removes the control. Invalid text remains visible and
    /// editing so the owner can refuse that lifecycle change.
    pub fn finish_edit(&mut self, cx: &mut Context<Self>) -> Result<(), NumericError> {
        self.text_mouse = None;
        self.text_click.clear();
        if let Some(press) = self.press.take() {
            if press.dragging {
                cx.emit(NumericControlEvent {
                    phase: NumericEventPhase::Commit,
                    value: self.draft,
                    gesture: self.active_gesture.clone(),
                });
            }
            self.active_gesture = None;
            cx.notify();
        }
        if !self.editing.is_editing() {
            return Ok(());
        }
        if self.deferred_text {
            if self.marked.is_some() {
                return Err(NumericError::InvalidText);
            } else if self.text_modified {
                self.request_deferred_text(cx);
                return Err(NumericError::InvalidText);
            }
            self.editing.0.set(false);
            self.marked = None;
            cx.notify();
            return Ok(());
        }
        let value = match self.finalized_text_value() {
            Ok(value) => value,
            Err(error) => {
                self.invalid_text = true;
                cx.notify();
                return Err(error);
            }
        };
        let old = self.value;
        if old != value {
            cx.emit(NumericControlEvent {
                phase: NumericEventPhase::Begin,
                value: old,
                gesture: None,
            });
            self.value = value;
            self.draft = value;
            cx.emit(NumericControlEvent {
                phase: NumericEventPhase::Change,
                value,
                gesture: None,
            });
            cx.emit(NumericControlEvent {
                phase: NumericEventPhase::Commit,
                value,
                gesture: None,
            });
        } else if self.commit_unchanged_text && self.text_modified {
            cx.emit(NumericControlEvent {
                phase: NumericEventPhase::Commit,
                value,
                gesture: None,
            });
        }
        self.text = self.spec.format(value)?;
        self.editing.0.set(false);
        self.text_modified = false;
        self.invalid_text = false;
        self.selected = 0..0;
        self.marked = None;
        cx.notify();
        Ok(())
    }

    /// Validate and read a local draft without clearing it or emitting events.
    /// Owners use this phase to admit the canonical edit before finalizing UI
    /// state; a refused edit therefore remains available for correction/retry.
    pub fn prepare_finish_edit_for_owner(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Result<Option<NumericControlFinalValue>, NumericError> {
        if let Some(press) = self.press {
            if press.dragging {
                return Ok(Some(NumericControlFinalValue {
                    explicit_edit: press.value_changed,
                    previous: press.start_value,
                    value: self.draft,
                    gesture: self.active_gesture.clone(),
                    formatted: self.spec.format(self.draft)?,
                }));
            }
            return Ok(Some(NumericControlFinalValue {
                explicit_edit: false,
                previous: self.value,
                value: self.value,
                gesture: None,
                formatted: self.spec.format(self.value)?,
            }));
        }
        if !self.editing.is_editing() {
            return Ok(None);
        }
        if self.deferred_text {
            if self.text_modified || self.marked.is_some() {
                return Err(NumericError::InvalidText);
            }
            return Ok(Some(NumericControlFinalValue {
                explicit_edit: false,
                previous: self.value,
                value: self.value,
                gesture: None,
                formatted: self.spec.format(self.value)?,
            }));
        }
        let value = match self.finalized_text_value() {
            Ok(value) => value,
            Err(error) => {
                self.invalid_text = true;
                cx.notify();
                return Err(error);
            }
        };
        let previous = self.value;
        let formatted = self.spec.format(value)?;
        Ok(Some(NumericControlFinalValue {
            explicit_edit: self.text_modified || previous != value,
            previous,
            value,
            gesture: None,
            formatted,
        }))
    }

    pub(super) fn finalized_text_value(&self) -> Result<f64, NumericError> {
        // A display codec may round the exact value. Focusing then leaving its
        // readout is not an edit; explicit input of that same text is an edit.
        if !self.text_modified && self.text == self.spec.format(self.value)? {
            Ok(self.value)
        } else {
            self.spec.parse_with_current(&self.text, Some(self.value))
        }
    }

    /// Complete a previously prepared draft after its owner admitted the
    /// canonical change. This method emits no subscription events.
    pub fn accept_finish_edit_for_owner(
        &mut self,
        finalized: NumericControlFinalValue,
        cx: &mut Context<Self>,
    ) {
        self.text_mouse = None;
        self.text_click.clear();
        if let Some(press) = self.press.take() {
            if press.dragging {
                self.value = finalized.value;
                self.draft = finalized.value;
                self.text = finalized.formatted;
            }
            self.editing.0.set(false);
            self.text_modified = false;
            self.invalid_text = false;
            self.selected = 0..0;
            self.marked = None;
            self.active_gesture = None;
            cx.notify();
            return;
        }
        if !self.editing.is_editing() {
            return;
        }
        self.value = finalized.value;
        self.draft = finalized.value;
        self.text = finalized.formatted;
        self.editing.0.set(false);
        self.text_modified = false;
        self.invalid_text = false;
        self.selected = 0..0;
        self.marked = None;
        cx.notify();
    }

    pub(crate) fn finish_text_edit_on_blur(&mut self, cx: &mut Context<Self>) {
        let _ = self.finish_edit(cx);
    }

    pub(super) fn escape(&mut self, _: &NumericEscape, _: &mut Window, cx: &mut Context<Self>) {
        self.invalidate_pending_text();
        self.text_mouse = None;
        self.text_click.clear();
        if self.press.is_some() {
            self.cancel(cx);
        } else if self.editing.is_editing() {
            self.editing.0.set(false);
            self.invalid_text = false;
            self.text = self.spec.format(self.value).unwrap_or_default();
            self.selected = 0..0;
            self.marked = None;
            cx.notify();
        }
    }

    pub(super) fn edit_left(&mut self, extend: bool, cx: &mut Context<Self>) {
        if self.editing.is_editing() {
            let cursor = self.cursor();
            let target = previous_boundary(&self.text, cursor);
            if extend {
                self.select_to(target);
            } else {
                self.move_to(target);
            }
            cx.notify();
        } else {
            self.emit_step(-1.0, false, cx);
        }
    }

    pub(super) fn edit_right(&mut self, extend: bool, cx: &mut Context<Self>) {
        if self.editing.is_editing() {
            let cursor = self.cursor();
            let target = next_boundary(&self.text, cursor);
            if extend {
                self.select_to(target);
            } else {
                self.move_to(target);
            }
            cx.notify();
        } else {
            self.emit_step(1.0, false, cx);
        }
    }

    pub(super) fn select_all(
        &mut self,
        _: &NumericSelectAll,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.invalidate_text_pointer_selection();
        cx.stop_propagation();
        if self.editing.is_editing() {
            self.selected = 0..self.text.len();
            self.reversed = false;
            cx.notify();
        }
    }

    pub(super) fn home(&mut self, _: &NumericHome, _: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        if self.editing.is_editing() {
            self.move_to(0);
            cx.notify();
        }
    }

    pub(super) fn end(&mut self, _: &NumericEnd, _: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        if self.editing.is_editing() {
            self.move_to(self.text.len());
            cx.notify();
        }
    }

    pub(super) fn backspace(
        &mut self,
        _: &NumericBackspace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.stop_propagation();
        if self.editing.is_editing() {
            self.delete_backward(window, cx);
        }
    }

    pub(super) fn delete(
        &mut self,
        _: &NumericDelete,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.stop_propagation();
        if self.editing.is_editing() {
            self.delete_forward(window, cx);
        }
    }

    pub(super) fn copy(&mut self, _: &NumericCopy, _: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        if self.editing.is_editing()
            && let Some(text) = self.text.get(self.selected.clone())
            && !text.is_empty()
        {
            cx.write_to_clipboard(ClipboardItem::new_string(text.to_owned()));
        }
    }

    pub(super) fn cut(&mut self, _: &NumericCut, window: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        if self.editing.is_editing()
            && let Some(text) = self.text.get(self.selected.clone())
            && !text.is_empty()
        {
            cx.write_to_clipboard(ClipboardItem::new_string(text.to_owned()));
            self.replace_text_in_range(None, "", window, cx);
        }
    }

    pub(super) fn paste(&mut self, _: &NumericPaste, window: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        if self.editing.is_editing()
            && let Some(text) = cx.read_from_clipboard().and_then(|item| item.text())
        {
            let clean = text.replace(['\n', '\r'], " ");
            self.replace_text_in_range(None, &clean, window, cx);
        }
    }

    pub(crate) fn value_string(&self) -> String {
        if self.editing.is_editing() {
            if self.deferred_text {
                self.text.clone()
            } else {
                self.style.text_case.present(&self.text)
            }
        } else if let Some((value, text)) = &self.external_display
            && *value == self.draft
        {
            text.clone()
        } else {
            self.style
                .text_case
                .present(&self.spec.format(self.draft).unwrap_or_default())
        }
    }

    pub(crate) fn value_text_case(&self) -> crate::TextCase {
        if self.deferred_text {
            crate::TextCase::Preserve
        } else {
            self.style.text_case
        }
    }

    pub(crate) fn cursor(&self) -> usize {
        if self.reversed {
            self.selected.start
        } else {
            self.selected.end
        }
    }

    pub(crate) fn invalidate_text_pointer_selection(&mut self) {
        self.text_mouse = None;
        self.text_click.clear();
    }

    pub(crate) fn move_to(&mut self, offset: usize) {
        self.caret_activity = self.caret_activity.wrapping_add(1);
        self.invalidate_text_pointer_selection();
        let offset = offset.min(self.text.len());
        self.selected = offset..offset;
        self.reversed = false;
    }

    pub(crate) fn select_to(&mut self, offset: usize) {
        self.caret_activity = self.caret_activity.wrapping_add(1);
        self.invalidate_text_pointer_selection();
        let offset = offset.min(self.text.len());
        if self.reversed {
            self.selected.start = offset;
        } else {
            self.selected.end = offset;
        }
        if self.selected.end < self.selected.start {
            self.reversed = !self.reversed;
            self.selected = self.selected.end..self.selected.start;
        }
    }

    pub(crate) fn delete_backward(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected.is_empty() {
            self.selected = previous_boundary(&self.text, self.cursor())..self.cursor();
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    pub(crate) fn delete_forward(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected.is_empty() {
            let cursor = self.cursor();
            self.selected = cursor..next_boundary(&self.text, cursor);
        }
        self.replace_text_in_range(None, "", window, cx);
    }
}

fn previous_boundary(text: &str, offset: usize) -> usize {
    text[..offset.min(text.len())]
        .char_indices()
        .last()
        .map_or(0, |(index, _)| index)
}

fn next_boundary(text: &str, offset: usize) -> usize {
    let offset = offset.min(text.len());
    text[offset..]
        .char_indices()
        .nth(1)
        .map_or(text.len(), |(index, _)| offset + index)
}
