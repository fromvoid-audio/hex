//! Desktop-injected optional off-callback tuning publisher.
use gpui::{App, Global};
#[derive(Clone)]
pub(crate) struct Publisher(pub hex_mts::Handle);
impl Global for Publisher {}
pub fn install_mts_publisher(cx: &mut App, handle: hex_mts::Handle) {
    cx.set_global(Publisher(handle));
}
