//! Desktop-injected optional native controller worker.
use gpui::{App, Global};
#[derive(Clone)]
pub(crate) struct ControllerWorker(pub hex_midi::Handle);
impl Global for ControllerWorker {}
pub fn install_controller_input(cx: &mut App, handle: hex_midi::Handle) {
    cx.set_global(ControllerWorker(handle));
}
