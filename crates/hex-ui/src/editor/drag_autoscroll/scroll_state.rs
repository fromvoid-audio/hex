//! Pure scroll state shared by captured drag payloads without GPUI view coupling.
use super::super::viewport::Viewport;
use std::{cell::Cell, rc::Rc, time::Instant};

pub(crate) type DragScroll = Rc<Cell<ScrollState>>;

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ScrollState {
    pub(crate) viewport: Option<Viewport>,
    pub(crate) last_tick: Option<Instant>,
    pub(crate) last_track_tick: Option<Instant>,
}
