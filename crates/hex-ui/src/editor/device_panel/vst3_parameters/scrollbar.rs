//! Pointer capture and geometry for the virtualized VST parameter list.
use super::{DeviceId, EditorView, Panel};
use crate::vertical_scrollbar::{self, Config, Target};
use gpui::{Context, Div, MouseDownEvent, Window};

pub(super) use crate::vertical_scrollbar::State;

pub(super) fn element(
    panel: &mut Panel,
    device: DeviceId,
    cx: &mut Context<EditorView>,
) -> gpui::Stateful<Div> {
    let rows = panel.matched.len();
    panel.scrollbar.begin_render_rows(rows);
    vertical_scrollbar::element(
        &panel.scrollbar,
        panel.scroll.clone(),
        Config {
            id: ("vst3-parameter-scrollbar", device.value()).into(),
            debug_selector: "vst3-parameter-scrollbar",
            generation: panel.scrollbar.generation(),
            rows,
            paint_unscrollable_thumb: false,
        },
        cx,
        move |view, event: &MouseDownEvent, window: &mut Window, cx| {
            cx.stop_propagation();
            window.prevent_default();
            vertical_scrollbar::begin_drag(view, event.position, window, cx, move |view| {
                let panel = view.device_panel.vst3_parameters.panels.get_mut(&device)?;
                let generation = panel.scrollbar.generation();
                let rows = panel.scrollbar.rows();
                Some(Target {
                    state: &mut panel.scrollbar,
                    scroll: &panel.scroll,
                    generation,
                    rows,
                })
            });
        },
    )
}
