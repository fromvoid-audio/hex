//! Shared native catalog/search surface. Service owns insertion and history;
//! this popup owns only its bounded query, list selection and return focus.
use super::*;
use hex_widgets::{TextInput, TextInputChanged};

const ROW_HEIGHT: f32 = 20.0;
gpui::actions!(hex_device_picker, [Previous, Next, Accept, Close]);
pub(super) struct Search {
    input: Entity<TextInput>,
    selected: usize,
    scroll: gpui::ScrollHandle,
    _subscription: Subscription,
    modal_scope: Option<(u64, DevicePanelOwner)>,
}
struct Entry {
    class: NativeDeviceClass,
    type_id: hex_lattice_contract::DeviceTypeId,
    label: String,
    selector: String,
}

pub(in crate::editor) fn bind_keys(cx: &mut App) {
    // Temporary search has no project editing commands. Native text copy,
    // paste and selection retain their more specific text-input bindings.
    cx.bind_keys([
        gpui::KeyBinding::new("up", Previous, Some("hex_device_picker")),
        gpui::KeyBinding::new("down", Next, Some("hex_device_picker")),
        gpui::KeyBinding::new("enter", Accept, Some("hex_device_picker")),
        gpui::KeyBinding::new("escape", Close, Some("hex_device_picker")),
    ]);
    cx.bind_keys(
        [
            "cmd-z",
            "cmd-shift-z",
            "cmd-n",
            "cmd-o",
            "cmd-s",
            "cmd-shift-s",
            "cmd-shift-e",
            "cmd-shift-t",
            "cmd-shift-m",
            "cmd-alt-t",
            "cmd-,",
            "tab",
            "shift-tab",
        ]
        .map(|key| gpui::KeyBinding::new(key, gpui::NoAction, Some("hex_device_picker"))),
    );
}
fn ensure(view: &mut EditorView, cx: &mut Context<EditorView>) {
    if view.device_panel.add_search.is_some() {
        return;
    }
    let input = cx.new(super::super::browser_input::device_search);
    let subscription = cx.subscribe(&input, |view, _, _: &TextInputChanged, cx| {
        if let Some(search) = &mut view.device_panel.add_search {
            search.selected = 0;
            search.scroll.set_offset(gpui::point(px(0.0), px(0.0)));
        }
        cx.notify();
    });
    view.device_panel.add_search = Some(Search {
        input,
        selected: 0,
        scroll: gpui::ScrollHandle::new(),
        _subscription: subscription,
        modal_scope: None,
    });
}
pub(super) fn open(view: &mut EditorView, window: &mut Window, cx: &mut Context<EditorView>) {
    ensure(view, cx);
    let search = view
        .device_panel
        .add_search
        .as_mut()
        .expect("search initialized above");
    search.selected = 0;
    search.modal_scope = view
        .device_panel
        .owner
        .map(|owner| (view.project_generation, owner));
    search.scroll.set_offset(gpui::point(px(0.0), px(0.0)));
    let input = search.input.clone();
    input.update(cx, |input, cx| input.set_text("", cx));
    input.read(cx).focus(window);
    view.device_panel.add_menu_open = true;
    cx.notify();
}
fn close(view: &mut EditorView, window: &mut Window, cx: &mut Context<EditorView>) {
    view.device_panel.add_menu_open = false;
    view.focus.focus(window);
    cx.notify();
}
fn entries(view: &EditorView, owner: DevicePanelOwner, query: &str) -> (Vec<Entry>, Vec<String>) {
    if matches!(owner, DevicePanelOwner::Source(_)) {
        return (Vec::new(), Vec::new());
    }
    let mut rows = Vec::new();
    let mut errors = Vec::new();
    let query = query.trim().to_lowercase();
    for class in [NativeDeviceClass::Effect, NativeDeviceClass::Instrument] {
        if class == NativeDeviceClass::Instrument && !matches!(owner, DevicePanelOwner::Track(_)) {
            continue;
        }
        let cached = match class {
            NativeDeviceClass::Effect => &view.device_panel.effect_catalog,
            NativeDeviceClass::Instrument => &view.device_panel.instrument_catalog,
        };
        let catalog = cached.clone().unwrap_or_else(|| {
            hex_engine::native_device_catalog_for_class(class).map_err(|error| error.to_string())
        });
        match catalog {
            Ok(catalog) => {
                for entry in catalog {
                    let kind = match class {
                        NativeDeviceClass::Effect => "effect",
                        NativeDeviceClass::Instrument => "instrument",
                    };
                    let type_name = entry.type_id.as_str().unwrap_or("invalid device");
                    if !format!("{} {kind} {type_name}", entry.name)
                        .to_lowercase()
                        .contains(&query)
                    {
                        continue;
                    }
                    let replacing = class == NativeDeviceClass::Instrument
                        && match owner {
                            DevicePanelOwner::Track(track) => view
                                .state
                                .as_ref()
                                .and_then(|state| state.project.track(track))
                                .is_some_and(|track| track.instrument().is_some()),
                            _ => false,
                        };
                    let prefix = match owner {
                        DevicePanelOwner::Track(_) => "native",
                        DevicePanelOwner::Main => "main",
                        DevicePanelOwner::Return(_) => "return",
                        DevicePanelOwner::Source(_) => continue,
                    };
                    rows.push(Entry {
                        class,
                        type_id: entry.type_id,
                        label: format!(
                            "{} · {}",
                            if replacing { "replace" } else { kind },
                            entry.name
                        ),
                        selector: format!("add-{prefix}-{kind}-{type_name}"),
                    });
                }
            }
            Err(error) => errors.push(format!("device catalog error · {error}")),
        }
    }
    (rows, errors)
}
fn activate(
    view: &mut EditorView,
    owner: DevicePanelOwner,
    class: NativeDeviceClass,
    type_id: hex_lattice_contract::DeviceTypeId,
    window: &mut Window,
    cx: &mut Context<EditorView>,
) {
    let Some(before) = device_add::before(view, owner) else {
        view.device_panel.message = Some("device insertion cancelled · chain changed".into());
        close(view, window, cx);
        return;
    };
    let Some(state) = view.state.as_ref() else {
        return;
    };
    let request = match owner {
        DevicePanelOwner::Track(track) => match (
            class,
            state
                .project
                .track(track)
                .and_then(|track| track.instrument())
                .map(hex_project::Device::id),
        ) {
            (NativeDeviceClass::Instrument, Some(device)) => Request::ReplaceNativeDevice {
                device,
                class,
                type_id: Box::new(type_id),
            },
            _ => Request::AddNativeDevice {
                track,
                class,
                type_id: Box::new(type_id),
                before,
            },
        },
        DevicePanelOwner::Main | DevicePanelOwner::Return(_) => {
            if class != NativeDeviceClass::Effect {
                return;
            }
            let bus = match owner {
                DevicePanelOwner::Main => EffectBusOwner::Main,
                DevicePanelOwner::Return(id) => EffectBusOwner::Return(id),
                // The outer match arm excludes Track.
                DevicePanelOwner::Track(_) | DevicePanelOwner::Source(_) => return,
            };
            bus.insert_request(view.project_generation, state.revision, before, type_id)
        }
        DevicePanelOwner::Source(_) => return,
    };
    if finalize_controls(view, cx) && view.enqueue_device_barrier(request) {
        view.device_panel.message = None;
        close(view, window, cx);
    }
    cx.notify();
}

pub(super) fn render(
    view: &mut EditorView,
    owner: DevicePanelOwner,
    cx: &mut Context<EditorView>,
) -> Stateful<Div> {
    ensure(view, cx);
    let search = view
        .device_panel
        .add_search
        .as_ref()
        .expect("search initialized above");
    let input = search.input.clone();
    super::super::browser_input::restyle(&input, cx);
    let (rows, errors) = entries(view, owner, input.read(cx).text());
    let selected = search.selected.min(rows.len().saturating_sub(1));
    let empty = rows.is_empty() && errors.is_empty();
    let list_height = ((rows.len() + errors.len()).max(1) as f32 * ROW_HEIGHT).min(240.0);
    let mut list = div()
        .id("device-picker-list")
        .debug_selector(|| "device-picker-list".into())
        .h(px(list_height))
        .min_h_0()
        .flex_shrink_0()
        .overflow_y_scroll()
        .track_scroll(&search.scroll)
        .flex()
        .flex_col();
    for (index, entry) in rows.into_iter().enumerate() {
        let Entry {
            class,
            type_id,
            label,
            selector,
        } = entry;
        list = list.child(
            div()
                .id(ElementId::Name(selector.clone().into()))
                .debug_selector(move || selector.clone())
                .h(px(ROW_HEIGHT))
                .flex_shrink_0()
                .px(px(5.0))
                .flex()
                .items_center()
                .text_xs()
                .cursor_pointer()
                .bg(rgb(theme::color(if index == selected {
                    theme::PaletteRole::Primary
                } else {
                    theme::PaletteRole::Raised
                })))
                .text_color(rgb(if index == selected {
                    theme::control_style().accent_text
                } else {
                    theme::color(theme::PaletteRole::Text)
                }))
                .hover(move |style| {
                    style.bg(rgb(theme::color(if index == selected {
                        theme::PaletteRole::Primary
                    } else {
                        theme::PaletteRole::Hover
                    })))
                })
                .child(theme::text(label))
                .on_click(cx.listener(move |view, _, window, cx| {
                    activate(view, owner, class, type_id, window, cx)
                })),
        );
    }
    if empty {
        list = list.child(
            menu_message("no matching devices").debug_selector(|| "device-picker-empty".into()),
        );
    }
    for (index, error) in errors.into_iter().enumerate() {
        list = list.child(
            menu_message(&error).debug_selector(move || format!("device-picker-error-{index}")),
        );
    }
    div()
        .id("device-add-menu")
        .debug_selector(|| "device-add-menu".into())
        .w(px(360.0))
        .max_w_full()
        .overflow_hidden()
        .occlude()
        .flex()
        .flex_col()
        .gap(px(2.0))
        .p(px(4.0))
        .rounded(px(3.0))
        .border_1()
        .border_color(rgb(theme::color(theme::PaletteRole::Border)))
        .bg(rgb(theme::color(theme::PaletteRole::Raised)))
        .key_context("hex_device_picker hex_text_editing")
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|view, _, window, cx| {
                if let Some(search) = &view.device_panel.add_search {
                    search.input.read(cx).focus(window);
                }
                cx.stop_propagation();
            }),
        )
        .child(
            div()
                .flex()
                .items_center()
                .h(px(20.0))
                .flex_shrink_0()
                .child(
                    div()
                        .flex_1()
                        .text_xs()
                        .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                        .child(theme::text("add device")),
                )
                .child(
                    hex_widgets::action(theme::text("×"), theme::action_style())
                        .id("device-picker-close")
                        .debug_selector(|| "device-picker-close".into())
                        .size(px(18.0))
                        .rounded_full()
                        .on_click(cx.listener(|view, _, window, cx| close(view, window, cx))),
                ),
        )
        .child(input)
        .child(list)
        .on_action(cx.listener(move |view, _: &Close, window, cx| {
            close(view, window, cx);
            cx.stop_propagation();
        }))
        .on_action(cx.listener(move |view, _: &Accept, window, cx| {
            if let Some(search) = view.device_panel.add_search.as_ref() {
                let (rows, _) = entries(view, owner, search.input.read(cx).text());
                if let Some(entry) = rows.get(search.selected.min(rows.len().saturating_sub(1))) {
                    activate(view, owner, entry.class, entry.type_id, window, cx);
                }
            }
            cx.stop_propagation();
        }))
        .on_action(cx.listener(move |view, _: &Previous, _, cx| {
            step(view, owner, false, list_height, cx);
            cx.stop_propagation();
        }))
        .on_action(cx.listener(move |view, _: &Next, _, cx| {
            step(view, owner, true, list_height, cx);
            cx.stop_propagation();
        }))
}
fn step(
    view: &mut EditorView,
    owner: DevicePanelOwner,
    forward: bool,
    list_height: f32,
    cx: &mut Context<EditorView>,
) {
    let Some(search) = view.device_panel.add_search.as_ref() else {
        return;
    };
    let (rows, _) = entries(view, owner, search.input.read(cx).text());
    let Some(search) = view.device_panel.add_search.as_mut() else {
        return;
    };
    search.selected = search.selected.min(rows.len().saturating_sub(1));
    search.selected = if forward {
        search
            .selected
            .saturating_add(1)
            .min(rows.len().saturating_sub(1))
    } else {
        search.selected.saturating_sub(1)
    };
    let top = -f32::from(search.scroll.offset().y);
    let y = search.selected as f32 * ROW_HEIGHT;
    let next = if y < top {
        y
    } else if y + ROW_HEIGHT > top + list_height {
        y + ROW_HEIGHT - list_height
    } else {
        top
    };
    search.scroll.set_offset(gpui::point(px(0.0), px(-next)));
    cx.notify();
}

/// One visible modal owner; a hidden detail pane never blocks the workspace.
pub(in crate::editor) fn is_open(view: &EditorView) -> bool {
    view.clip_view_open
        && view.device_panel_active
        && view.device_panel.add_menu_is_open(view.state.as_deref())
}
pub(in crate::editor) fn overlay(
    view: &mut EditorView,
    window: &mut Window,
    cx: &mut Context<EditorView>,
) -> Option<Stateful<Div>> {
    if !is_open(view) {
        if let Some(search) = &mut view.device_panel.add_search {
            search.modal_scope = None;
        }
        return None;
    }
    let owner = view.device_panel.owner?;
    ensure(view, cx);
    let scope = (view.project_generation, owner);
    let search = view.device_panel.add_search.as_mut()?;
    if search.modal_scope.is_some_and(|current| current != scope) {
        search.modal_scope = None;
        close(view, window, cx);
        return None;
    }
    if search.modal_scope.is_none() {
        search.modal_scope = Some(scope);
        search.input.read(cx).focus(window);
    }
    let card = render(view, owner, cx);
    Some(
        div()
            .id("device-picker-modal")
            .debug_selector(|| "device-picker-modal".into())
            .absolute()
            .inset_0()
            .occlude()
            .flex()
            .items_center()
            .justify_center()
            .p_4()
            .bg(gpui::rgba(0x00000077))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|view, _, window, cx| {
                    close(view, window, cx);
                    cx.stop_propagation();
                }),
            )
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Middle, |_, _, cx| cx.stop_propagation())
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .child(card),
    )
}
