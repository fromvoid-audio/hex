//! Held input only: the Window lease does not own an admitted edit or receipt.
//! Bind callbacks to the displayed clip/generation/revision; edit helpers retain
//! canonical validation and Pending retains frozen geometry across release/ACK.
//! Source loss discards only the still-held gesture, never an admitted preview.
use super::{EditorView, Kind};
use gpui::{Context, CursorStyle, MouseButton, MouseMoveEvent, MouseUpEvent, Window};

impl Kind {
    pub(super) fn pointer_cursor(self) -> CursorStyle {
        match self {
            Self::Select => CursorStyle::Arrow,
            Self::PlayStart { .. } => CursorStyle::ResizeLeftRight,
            Self::RangeMove { .. }
            | Self::FreeRangeMove { .. }
            | Self::Move(_)
            | Self::FreeMove { .. }
            | Self::Duplicate { .. } => CursorStyle::OpenHand,
            Self::Leading(_) => CursorStyle::ResizeLeftBracket,
            Self::Trailing(_) => CursorStyle::ResizeRightBracket,
            Self::Fade(fade) => fade.pointer_cursor(),
            Self::Gain { .. } => CursorStyle::ResizeUpDown,
        }
    }
}

impl EditorView {
    pub(super) fn audio_event_pointer_cursor(&self, kind: Kind) -> CursorStyle {
        if matches!(kind, Kind::Select) {
            self.time_selection_pointer_cursor(kind.pointer_cursor())
        } else {
            kind.pointer_cursor()
        }
    }

    pub(super) fn capture_audio_event_pointer(
        &self,
        kind: Kind,
        window: &mut Window,
        cx: &Context<Self>,
    ) -> bool {
        let Some(source) = self.audio_event_editor.owner else {
            return false;
        };
        let revision = self.audio_event_editor.revision;
        let owner = cx.entity().downgrade();
        let cancelled = owner.clone();
        window.capture_pointer(
            cx.entity_id(),
            MouseButton::Left,
            self.audio_event_pointer_cursor(kind),
            move |event, window, cx| {
                let moved = event.downcast_ref::<MouseMoveEvent>();
                let released = event
                    .downcast_ref::<MouseUpEvent>()
                    .filter(|event| event.button == MouseButton::Left);
                let Some(position) = moved
                    .map(|event| event.position)
                    .or_else(|| released.map(|event| event.position))
                else {
                    return;
                };
                let _ = owner.update(cx, |view, cx| {
                    if view.audio_event_editor.owner != Some(source)
                        || view.audio_event_editor.revision != revision
                    {
                        return;
                    }
                    // Existing pointer operations validate canonical source/revision.
                    // This boundary guards the captured display owner and focus.
                    if !view.audio_event_focus.is_focused(window) {
                        if view.audio_event_editor.gesture.take().is_some() {
                            cx.notify();
                        }
                        view.audio_event_editor.fade_key_held = false;
                        return;
                    }
                    let Some(gesture) = view.audio_event_editor.gesture.as_ref() else {
                        return;
                    };
                    if !f32::from(position.x).is_finite() || !f32::from(position.y).is_finite() {
                        view.audio_event_editor.gesture = None;
                        cx.notify();
                        return;
                    }
                    let fade = matches!(gesture.kind, Kind::Fade(_));
                    let gain = matches!(gesture.kind, Kind::Gain { .. });
                    if let Some(event) = moved {
                        if fade {
                            view.update_audio_event_fade_pointer(
                                event.position,
                                event.modifiers.platform,
                                window,
                                cx,
                            );
                        } else if gain {
                            view.event_update_gain(
                                f32::from(event.position.y),
                                event.modifiers.shift,
                            );
                            cx.notify();
                        } else {
                            view.event_update_pointer(
                                f32::from(event.position.x),
                                event.modifiers.platform,
                            );
                            cx.notify();
                        }
                    } else if let Some(event) = released {
                        if fade {
                            view.finish_audio_event_fade_pointer(
                                event.position,
                                event.modifiers.platform,
                                window,
                                cx,
                            );
                        } else if gain {
                            view.event_update_gain(
                                f32::from(event.position.y),
                                event.modifiers.shift,
                            );
                            if let Some(gesture) = view.audio_event_editor.gesture.take()
                                && let Some(operation) = gesture.operation
                            {
                                view.event_commit(operation);
                            }
                            cx.notify();
                        } else {
                            view.event_release_pointer(
                                f32::from(event.position.x),
                                event.modifiers.platform,
                            );
                            cx.notify();
                        }
                    }
                });
            },
            move |window, cx| {
                let _ = cancelled.update(cx, |view, cx| {
                    if view.audio_event_editor.owner == Some(source)
                        && view.audio_event_editor.revision == revision
                        && view.audio_event_editor.gesture.take().is_some()
                    {
                        if !view.audio_event_focus.is_focused(window) {
                            view.audio_event_editor.fade_key_held = false;
                        }
                        cx.notify();
                    }
                });
            },
            cx,
        )
    }
}
