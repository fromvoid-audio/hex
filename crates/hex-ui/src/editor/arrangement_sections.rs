//! Main-lane section display and captured, stamped user intents.
//!
//! Project owns section identity, validation and song-wide permutation. This
//! module projects marker geometry and gesture-owned feedback. Release transfers immutable paint to
//! an ordinary FIFO receipt; Escape/blur cancel held input, never admitted work.
//! Option-Command before or during capture latches marker-only intent. Command
//! alone live-inverts snapping; Control remains a snap alias, including marker
//! edits. Option alone copies song content at structural edges.
//! Leaving the lane retains the last valid preview. Body moves/copies use the
//! existing Window pointer lease; its painted cursor follows the held command,
//! while released preview custody remains independent of input capture.
//! Five-pixel inner edges resize one section; a touching seam resizes both, or
//! the left marker's end under Option-Command. Paint and input share native bracket cursors.
//! Shift-click selects the song-order range from the last plain-clicked anchor;
//! Command+Shift-click toggles membership and moves the anchor. Header drags own
//! section edits; ordinary lower-body drags use Main's all-track range owner,
//! including tracks hidden by section focus. A double press in empty lane space
//! paints a section at every height; existing sections retain their input owner.
use super::{
    EditorMode, EditorView, ViewState, arrangement_map,
    musical_time::SnapGrid,
    pending_edit::{Disposition, Receipt},
    viewport::Viewport,
};
use crate::{
    context_menu::{MenuItem, PopupMenu},
    section_appearance,
    section_hit::{self, INNER_EDGE_WIDTH, SHARED_EDGE_RADIUS},
    section_input, section_reorder, section_resize, section_selection, theme,
};
use gpui::{
    AnyElement, AppContext, Context, Entity, Subscription, Window, div, prelude::*, px, relative,
    rgb,
};
use hex_project::{ArrangementSection, BeatTime, Command, ElementColor, SectionId, TempoMap};
use hex_session::service::{ArrangementEditViewObservation, Event, Request};
use hex_widgets::{
    RectangleCapture, RectanglePart, RectangleSelection, RectangleSelectionPolicy, TextInput,
};
use std::sync::Arc;

#[cfg(all(test, feature = "ui-test-support"))]
mod beyond_end_tests;
mod clipboard;
#[cfg(all(test, feature = "ui-test-support"))]
mod drop_tests;

#[derive(Clone, Copy, PartialEq, Eq)]
enum SectionEdge {
    Single(SectionId, hex_project::TrimEdge),
    Shared(SectionId, SectionId),
}

#[derive(Clone)]
struct Marker {
    id: Option<SectionId>,
    name: String,
    color: ElementColor,
    start: BeatTime,
    end: BeatTime,
}
impl From<&ArrangementSection> for Marker {
    fn from(s: &ArrangementSection) -> Self {
        Self {
            id: (s.id().value() != 0).then_some(s.id()),
            name: s.name().into(),
            color: s.color(),
            start: s.start(),
            end: s.end(),
        }
    }
}
#[derive(Clone)]
struct Display {
    markers: Vec<Marker>,
    tempo: TempoMap,
    // Owned gesture feedback, frozen with the exact released display. Ordinary
    // hover resumes elsewhere without repainting this receipt's final fill.
    pointer_hover: Option<BeatTime>,
    // Sections under the held edge or body keep one hovered fill for the whole
    // gesture and its receipt; a snapped edge crossing the pointer must not
    // toggle it.
    held: [Option<SectionId>; 2],
    marker_only: bool,
    drop_feedback: Option<DropDisplay>,
}
#[derive(Clone)]
struct DropDisplay {
    feedback: section_reorder::DropFeedback,
    // Earlier FIFO completions may install a different clock while this exact
    // receipt still owns paint. Never reproject its original target with it.
    tempo: TempoMap,
}
impl Display {
    fn pointer_hover_at(
        &self,
        viewport: Viewport,
        [left, top, width, height]: [f32; 4],
        (x, y): (f32, f32),
    ) -> Option<BeatTime> {
        let right = left + width;
        let bottom = top + height;
        if [left, top, width, height, right, bottom, x, y]
            .into_iter()
            .any(|value| !value.is_finite())
            || width <= 0.0
            || height <= 0.0
            || x < left
            || x > right
            || y < top
            || y > bottom
        {
            return None;
        }
        let frame = super::seek::pointer_to_frame(x, left, width, viewport)?;
        self.tempo.frame_to_beat(frame).ok()
    }
}
#[derive(Clone, Copy)]
enum Mode {
    Paint,
    Start(SectionId),
    End(SectionId),
    Move(SectionId),
    Reorder(SectionId),
    Shared {
        left: SectionId,
        right: SectionId,
        marker_only: bool,
    },
    ContentStart(SectionId),
    ContentEnd(SectionId),
}
impl Mode {
    fn marker_only(self) -> bool {
        matches!(
            self,
            Self::Start(_)
                | Self::End(_)
                | Self::Move(_)
                | Self::Shared {
                    marker_only: true,
                    ..
                }
        )
    }

    fn held(self) -> [Option<SectionId>; 2] {
        match self {
            Mode::Paint => [None; 2],
            Mode::Shared { left, right, .. } => [Some(left), Some(right)],
            Mode::Start(id)
            | Mode::End(id)
            | Mode::Move(id)
            | Mode::Reorder(id)
            | Mode::ContentStart(id)
            | Mode::ContentEnd(id) => [Some(id), None],
        }
    }
}
struct Gesture {
    launch_eligible: bool,
    gap_appearance: Option<(String, ElementColor)>,
    generation: u64,
    revision: u64,
    viewport: Viewport,
    snap: SnapGrid,
    layout: [f32; 4],
    anchor: BeatTime,
    last_pointer: (f32, f32),
    capture: RectangleCapture<(u64, Option<SectionId>)>,
    original: Option<Marker>,
    mode: Mode,
    display: Display,
    command: Option<Command>,
}
struct Pending {
    generation: u64,
    revision: u64,
    command: Command,
    display: Display,
    // Projected clip rows at submission, superseded by an exact copy view
    // when available. Retired with the receipt and canonical installation.
    layout: Option<Arc<ViewState>>,
    // The created ID belongs to the exact committed copy observation.
    copied: Option<SectionId>,
    // Cut commits its captured snapshot and cursor only when deletion succeeds.
    cut_clipboard: Option<clipboard::Captured>,
    cut_cursor: Option<BeatTime>,
    receipt: Receipt,
    selection_intent_epoch: Option<u64>,
    /// True when released while an unrelated worker request (e.g. clip
    /// duplication) was in flight; waits for the worker queue to drain before
    /// submitting `EditArrangementSection` against the resulting revision.
    deferred_submission: bool,
}
struct Name {
    id: SectionId,
    generation: u64,
    revision: u64,
    input: Entity<TextInput>,
    _blur: Subscription,
}
/// View-only beat position; a revision change requires a fresh lane click.
#[derive(Clone, Copy)]
struct SectionCursor {
    beat: BeatTime,
    generation: u64,
    revision: u64,
}
#[derive(Default)]
pub(super) struct Sections {
    pub(super) selected: Option<SectionId>,
    selection: Vec<SectionId>,
    // Shift ranges start here; ignored once it leaves the selection.
    anchor: Option<SectionId>,
    cursor: Option<SectionCursor>,
    clipboard: Option<clipboard::Captured>,
    gesture: Option<Gesture>,
    pending: Option<Pending>,
    // At most one keyboard duplicate may follow a committed pending copy.
    ahead: Option<Pending>,
    // One keyboard/menu follow-up typed while an edit is in flight (latest
    // wins). It is validated and submitted against the acknowledged revision.
    queued: Option<Queued>,
    name: Option<Name>,
    launch_focus: std::cell::RefCell<std::collections::BTreeMap<SectionId, gpui::FocusHandle>>,
}
struct Queued {
    generation: u64,
    command: Command,
    /// Optional frozen pointer-release preview retained until `submit_queued_section`
    /// hands it off to `sections.pending`, preventing release flicker.
    display: Option<Display>,
}
/// Visible refusal for section input that cannot wait for an in-flight edit.
const SECTION_BUSY: &str = "applying section edit · try again when it finishes";
impl Sections {
    pub(super) fn selected_ids(&self) -> &[SectionId] {
        if self.selected.is_some_and(|id| self.selection.contains(&id)) {
            &self.selection
        } else {
            self.selected.as_slice()
        }
    }
    pub(super) fn held(&self) -> bool {
        self.gesture.is_some()
    }
    pub(super) fn pending(&self) -> bool {
        self.pending.is_some()
    }
    pub(super) fn naming(&self) -> bool {
        self.name.is_some()
    }
}
#[derive(Clone)]
pub(super) struct MenuAction {
    generation: u64,
    revision: u64,
    section: Option<SectionId>,
    kind: MenuKind,
}
#[derive(Clone)]
enum MenuKind {
    EditSceneInConsole(hex_project::SceneId),
    Rename,
    Color(ElementColor),
    Remove,
    Delete,
    Duplicate,
    Cut,
    Copy,
    Paste,
    Merge,
}

mod geometry;
mod gestures;
mod render;
use geometry::{copied_section_source, same_command, section_preview_span};

#[cfg(all(test, not(feature = "ui-test-support")))]
pub(super) mod tests;

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "arrangement_sections/ui_tests.rs"]
pub(super) mod tests;
