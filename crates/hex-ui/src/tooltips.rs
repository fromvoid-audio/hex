//! App-session help presentation, separate from functional identification.
//! Names, values, truncated choices and diagnostics remain available. Help is
//! optional; mixed controls retain their explicit name while help is disabled.
//! MiniTooltip/ClipTooltip callers must select a kind explicitly. Demo transport
//! actions, browser icons, menus, errors/recovery and widget-owned input labels,
//! panel names, selector choices and meter readings are functional identification.
//! Those diagnostic/value widgets keep their existing always-visible presentation.
//! This state never edits a project or persists a project-schema preference.
use crate::theme;
use gpui::{
    AccessibilityAction, AccessibilityDescriptor, AccessibilityId, AccessibilityLifetime,
    AccessibilityValue, App, Div, FocusHandle, Global, IntoElement, MouseButton, RenderOnce,
    Window, div, prelude::*, px, rgb,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Identification,
    Help,
    Control(gpui::SharedString),
}
impl Kind {
    pub(crate) fn control(name: impl Into<gpui::SharedString>) -> Self {
        Self::Control(name.into())
    }
}

#[derive(Clone, Copy)]
struct Settings {
    enabled: bool,
    revision: u64,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: false,
            revision: 1,
        }
    }
}
impl Global for Settings {}

fn settings(cx: &App) -> Settings {
    cx.try_global::<Settings>().copied().unwrap_or_default()
}
pub(crate) fn enabled(cx: &App) -> bool {
    settings(cx).enabled
}
fn toggle(cx: &mut App) -> bool {
    let previous = settings(cx);
    let Some(revision) = previous.revision.checked_add(1) else {
        return false;
    };
    cx.set_global(Settings {
        enabled: !previous.enabled,
        revision,
    });
    // Active tooltip views are ordinary uncached GPUI views too. Redraw them
    // immediately, including keyboard activation with a stationary pointer.
    cx.refresh_windows();
    true
}

pub(crate) fn view(kind: &Kind, description: &str, cx: &App) -> Div {
    let help = enabled(cx);
    let text = match kind {
        Kind::Help if !help => return div(),
        Kind::Control(name) if !help => name.as_ref(),
        _ => description,
    };
    let selector = if help && kind != &Kind::Identification {
        "help-tooltip"
    } else {
        "identification-tooltip"
    };
    div()
        .debug_selector(move || selector.into())
        .px_2()
        .py_1()
        .border_1()
        .border_color(rgb(theme::color(theme::PaletteRole::Border)))
        .bg(rgb(theme::color(theme::PaletteRole::Raised)))
        .font_family(theme::FONT_FAMILY)
        .text_xs()
        .text_color(rgb(theme::color(theme::PaletteRole::Text)))
        .child(theme::text(text))
}

pub(crate) fn bind_keys(cx: &mut App) {
    // Keep activation in this control's key handler (including held-key refusal),
    // ahead of the editor's Space transport and Enter launch actions.
    cx.bind_keys(
        ["enter", "space"]
            .map(|key| gpui::KeyBinding::new(key, gpui::NoAction, Some("hex_help_tooltip_toggle"))),
    );
}

struct ButtonState {
    focus: FocusHandle,
    lifetime: AccessibilityLifetime,
}

#[derive(IntoElement)]
pub(crate) struct HelpToggle;
impl RenderOnce for HelpToggle {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = window.use_keyed_state("help-tooltip-toggle-state", cx, |_, cx| ButtonState {
            focus: cx.focus_handle().tab_stop(true).tab_index(0),
            lifetime: AccessibilityLifetime::default(),
        });
        let current = settings(cx);
        let focus = state.read(cx).focus.clone();
        let lifetime = state.read(cx).lifetime.weak();
        let owner = state.downgrade();
        hex_widgets::toggle_action(
            theme::text("?"),
            theme::device_segment_style(current.enabled),
        )
        .id("help-tooltip-toggle")
        .debug_selector(|| "help-tooltip-toggle".into())
        .size(px(20.0))
        .flex_shrink_0()
        .text_size(px(12.0))
        .track_focus(&focus)
        .key_context("hex_help_tooltip_toggle")
        .focus(|style| {
            style
                .border_1()
                .border_color(rgb(theme::color(theme::PaletteRole::Primary)))
        })
        .tooltip(|_, cx| cx.new(|_| HelpStateTooltip).into())
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(|_, _, cx| {
            cx.stop_propagation();
            toggle(cx);
        })
        .on_key_down(|event, window, cx| {
            let modifiers = event.keystroke.modifiers;
            if modifiers.control || modifiers.alt || modifiers.platform {
                return;
            }
            if event.keystroke.key == "tab" {
                if modifiers.shift {
                    window.focus_prev()
                } else {
                    window.focus_next()
                }
                cx.stop_propagation();
            } else if !modifiers.shift && matches!(event.keystroke.key.as_str(), "enter" | "space")
            {
                if !event.is_held {
                    toggle(cx);
                }
                cx.stop_propagation();
            }
        })
        .accessibility(
            AccessibilityDescriptor {
                id: AccessibilityId(state.entity_id().as_u64()),
                revision: current.revision,
                lifetime,
                name: theme::text("help tooltips"),
                value: AccessibilityValue::Toggle(current.enabled),
                enabled: true,
                focused: focus.is_focused(window),
                status: None,
            },
            move |request, window, cx| {
                if settings(cx).revision != request.revision {
                    return false;
                }
                owner
                    .update(cx, |state, cx| match request.action {
                        AccessibilityAction::Focus => {
                            state.focus.focus(window);
                            true
                        }
                        AccessibilityAction::Press => toggle(cx),
                        _ => false,
                    })
                    .unwrap_or(false)
            },
        )
    }
}

struct HelpStateTooltip;
impl gpui::Render for HelpStateTooltip {
    fn render(&mut self, _: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        view(
            &Kind::Identification,
            if enabled(cx) {
                "help tooltips: on"
            } else {
                "help tooltips: off"
            },
            cx,
        )
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests {
    use super::*;
    use gpui::{Context, Modifiers, Render, TestAppContext, VisualTestContext};

    struct Tooltip(Kind);
    impl Render for Tooltip {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            view(&self.0, "drag to adjust · shift for fine control", cx)
        }
    }
    struct Fixture;
    impl Render for Fixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .flex()
                .gap_4()
                .child(crate::tooltips::HelpToggle)
                .children(
                    [
                        ("help-only", Kind::Help),
                        ("identified", Kind::Identification),
                        ("mixed", Kind::control("fine adjustment")),
                    ]
                    .map(|(id, kind)| {
                        div()
                            .id(id)
                            .debug_selector(move || id.into())
                            .size(px(60.0))
                            .child(theme::text(id))
                            .tooltip(move |_, cx| cx.new(|_| Tooltip(kind.clone())).into())
                    }),
                )
        }
    }
    fn fixture(cx: &mut TestAppContext) -> VisualTestContext {
        cx.update(bind_keys);
        let handle = *cx.add_window(|_, _| Fixture);
        let visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        visual
    }
    fn hover(visual: &mut VisualTestContext, selector: &'static str) {
        let center = visual.debug_bounds(selector).unwrap().center();
        visual.simulate_mouse_move(center, None, Modifiers::default());
        visual.run_until_parked();
        visual
            .executor()
            .advance_clock(std::time::Duration::from_secs(1));
        visual.run_until_parked();
    }
    fn node(visual: &mut VisualTestContext) -> gpui::AccessibilityDescriptor {
        visual.update(|window, _| {
            window
                .accessibility_nodes()
                .find(|node| node.descriptor.name.as_ref() == "help tooltips")
                .expect("help semantic toggle")
                .descriptor
                .clone()
        })
    }
    fn request(
        visual: &mut VisualTestContext,
        node: &gpui::AccessibilityDescriptor,
        action: AccessibilityAction,
    ) -> bool {
        visual.update(|window, cx| {
            window.perform_accessibility_request(
                &gpui::AccessibilityRequest {
                    id: node.id,
                    revision: node.revision,
                    action,
                },
                cx,
            )
        })
    }
    #[gpui::test]
    fn default_off_retains_identification_and_mixed_control_names(cx: &mut TestAppContext) {
        let mut visual = fixture(cx);
        assert!(!visual.update(|_, cx| enabled(cx)));
        assert_eq!(node(&mut visual).value, AccessibilityValue::Toggle(false));
        hover(&mut visual, "help-only");
        assert!(visual.debug_bounds("help-tooltip").is_none());
        assert!(visual.debug_bounds("identification-tooltip").is_none());
        for target in ["identified", "mixed"] {
            hover(&mut visual, target);
            assert!(
                visual.debug_bounds("identification-tooltip").is_some(),
                "{target}"
            );
            assert!(visual.debug_bounds("help-tooltip").is_none());
        }
    }
    #[gpui::test]
    fn keyboard_and_accessibility_refresh_visible_help_without_pointer_motion(
        cx: &mut TestAppContext,
    ) {
        let mut visual = fixture(cx);
        let initial = node(&mut visual);
        assert!(request(&mut visual, &initial, AccessibilityAction::Focus));
        visual.simulate_keystrokes("space");
        visual.run_until_parked();
        assert!(visual.update(|_, cx| enabled(cx)));
        assert!(
            !request(&mut visual, &initial, AccessibilityAction::Press),
            "reject stale activation"
        );
        hover(&mut visual, "help-only");
        assert!(visual.debug_bounds("help-tooltip").is_some());
        // Keyboard focus stayed on the toggle while the pointer stayed on help.
        visual.simulate_keystrokes("enter");
        visual.run_until_parked();
        assert!(!visual.update(|_, cx| enabled(cx)));
        assert!(visual.debug_bounds("help-tooltip").is_none());
        // Enabling an already hovered, empty help tooltip redraws it too.
        let current = node(&mut visual);
        assert!(request(&mut visual, &current, AccessibilityAction::Press));
        visual.run_until_parked();
        assert!(visual.debug_bounds("help-tooltip").is_some());
        hover(&mut visual, "identified");
        assert!(visual.debug_bounds("identification-tooltip").is_some());
    }
}
