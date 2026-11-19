//! Disposable rectangle and time-range samples with an in-memory section project.
//! The section fixture uses project-owned edits and shared previews without audio
//! services. Pending paint advances only through an explicit terminal action.
#[path = "section_fixture.rs"]
mod section_fixture;
use crate::theme;
use gpui::{
    Bounds, Context, FocusHandle, Focusable, Modifiers, MouseButton, Pixels, Render, Subscription,
    Window, div, prelude::*, px, rgb,
};
use hex_project::{ArrangementSection, BeatTime, Command, Project};
use hex_widgets::{
    RangeSelectionTarget, RangeSelectionZones, RectangleCapture, RectangleEdges, RectanglePart,
    RectangleSelection, RectangleSelectionPolicy, rectangle_body, rectangle_edge,
    rectangle_surface,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Clip,
    Section,
    TimeRange,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Target {
    Rectangle(usize),
    PaintRange,
    SectionRange,
    SectionPaint,
}

enum Pending {
    Local {
        display: Vec<[f32; 2]>,
    },
    Section {
        display: Vec<[f32; 2]>,
        sections: Vec<ArrangementSection>,
        command: Box<Command>,
        source: Box<Project>,
        receipt: crate::pending_edit::Receipt,
        hover: crate::section_surface::Hover,
        feedback: Option<crate::section_reorder::DropFeedback>,
    },
}
impl Pending {
    fn display(&self) -> Vec<[f32; 2]> {
        match self {
            Self::Local { display } | Self::Section { display, .. } => display.clone(),
        }
    }
}

pub(super) struct RectangleDemo {
    kind: Kind,
    id: String,
    focus: FocusHandle,
    blur: Option<Subscription>,
    bounds: Option<Bounds<Pixels>>,
    canonical: Vec<[f32; 2]>,
    preview: Vec<[f32; 2]>,
    capture: Option<RectangleCapture<Target>>,
    pending: Option<Pending>,
    section_fixture: Option<section_fixture::Fixture>,
    section_preview: Vec<ArrangementSection>,
    section_command: Option<Command>,
    section_feedback: Option<crate::section_reorder::DropFeedback>,
    section_error: Option<&'static str>,
    section_marker_latched: bool,
    section_gap_appearance: Option<(String, hex_project::ElementColor)>,
    section_focus: bool,
    section_anchor_beat: Option<BeatTime>,
    section_last_pointer: Option<(f32, f32)>,
    section_released_hover: crate::section_surface::Hover,
    section_paint: Option<(String, hex_project::ElementColor)>,
    generation: u64,
    range: Option<[f32; 2]>,
    range_before: Option<[f32; 2]>,
    selected: Vec<usize>,
    selection_primary: Option<usize>,
    selection_anchor: Option<usize>,
    enabled: bool,
    status: &'static str,
}
impl RectangleDemo {
    pub fn new(kind: Kind, id: String, cx: &Context<Self>) -> Self {
        let (section_fixture, initial_error) = if kind == Kind::Section {
            match section_fixture::Fixture::new() {
                Ok(fixture) => (Some(fixture), None),
                Err(error) => (None, Some(error.0)),
            }
        } else {
            (None, None)
        };
        let canonical = if kind == Kind::Section {
            section_fixture
                .as_ref()
                .map_or_else(Vec::new, section_fixture::Fixture::display)
        } else {
            vec![[0.1, 0.5], [0.7, 0.71]]
        };
        let section_preview = section_fixture.as_ref().map_or_else(Vec::new, |fixture| {
            fixture.project().arrangement_sections().to_vec()
        });
        let status = initial_error.unwrap_or("ready");
        Self {
            kind,
            id,
            focus: cx.focus_handle(),
            blur: None,
            bounds: None,
            canonical: canonical.clone(),
            preview: canonical.clone(),
            capture: None,
            pending: None,
            section_fixture,
            section_preview,
            section_command: None,
            section_feedback: None,
            section_error: None,
            section_marker_latched: false,
            section_gap_appearance: None,
            section_focus: false,
            section_anchor_beat: None,
            section_last_pointer: None,
            section_released_hover: crate::section_surface::Hover::default(),
            section_paint: None,
            generation: 1,
            range: None,
            range_before: None,
            selected: if kind == Kind::TimeRange {
                vec![0]
            } else {
                Vec::new()
            },
            selection_primary: None,
            selection_anchor: None,
            enabled: true,
            status,
        }
    }
    fn edges(&self) -> RectangleEdges {
        RectangleEdges {
            width: if self.kind == Kind::Section { 5.0 } else { 4.0 },
            minimum_body: if self.kind == Kind::Section { 0.0 } else { 1.0 },
        }
    }
    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        if let Some(capture) = self.capture.take() {
            if capture.key == Target::SectionRange {
                self.range = self.range_before.take();
            } else {
                self.preview = self.canonical.clone();
            }
            let _ = capture.cancel();
            self.section_command = None;
            self.section_feedback = None;
            self.section_error = None;
            self.section_marker_latched = false;
            self.section_gap_appearance = None;
            self.section_anchor_beat = None;
            self.section_last_pointer = None;
            self.section_released_hover = crate::section_surface::Hover::default();
            self.section_paint = None;
            self.status = "canceled held input";
            cx.notify();
        }
    }
    pub fn set_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.cancel(cx);
        self.enabled = enabled;
        cx.notify();
    }
    fn begin(
        &mut self,
        x: f32,
        y: f32,
        double: bool,
        modifiers: Modifiers,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let edit = crate::section_input::EditModifiers::from(modifiers);
        if !self.enabled
            || self.pending.is_some()
            || (self.kind == Kind::Section && self.section_fixture.is_none())
        {
            return false;
        }
        let Some(bounds) = self.bounds else {
            return false;
        };
        let left = f32::from(bounds.left());
        let width = f32::from(bounds.size.width);
        if y < f32::from(bounds.top()) || y >= f32::from(bounds.bottom()) {
            return false;
        }
        if self.kind == Kind::TimeRange {
            if !x.is_finite() || x < left || x >= left + width {
                return false;
            }
            let Some(zones) = RangeSelectionZones::new(bounds.size.height.into(), 0.0) else {
                return false;
            };
            let [start, end] = self.canonical[0];
            let existing = x >= left + start * width && x < left + end * width;
            let Some(target) = zones.target(y - f32::from(bounds.top()), existing, modifiers.shift)
            else {
                return false;
            };
            let target = match target {
                RangeSelectionTarget::Move => Target::Rectangle(0),
                RangeSelectionTarget::Paint => Target::PaintRange,
            };
            self.capture = RectangleCapture::begin(target, RectanglePart::Body, x, true);
            self.preview = self.canonical.clone();
            self.selected = vec![0];
            self.status = "held range preview";
            self.focus.focus(window);
            cx.notify();
            return true;
        }
        let mut hit = if self.kind == Kind::Section {
            self.section_fixture.as_ref().and_then(|fixture| {
                let sections = fixture
                    .project()
                    .arrangement_sections()
                    .iter()
                    .enumerate()
                    .map(|(index, section)| (index, section.start(), section.end()));
                crate::section_hit::hit(sections, x, |beat| {
                    Some(left + width * fixture.position(beat))
                })
                .map(|hit| match hit {
                    crate::section_hit::Hit::Leading(index) => (index, RectanglePart::Leading),
                    crate::section_hit::Hit::Trailing(index) => (index, RectanglePart::Trailing),
                    crate::section_hit::Hit::Seam(left, _) => (left, RectanglePart::Seam),
                })
            })
        } else {
            None
        };
        if hit.is_none() {
            for (index, [start, end]) in self.canonical.iter().copied().enumerate() {
                let start = left + start * width;
                let end = left + end * width;
                if let Some(part) = self.edges().hit(x, start, end) {
                    if self.kind == Kind::Section && part != RectanglePart::Body {
                        continue;
                    }
                    if self.kind == Kind::Clip
                        && part == RectanglePart::Body
                        && y > f32::from(bounds.top()) + 14.0
                    {
                        continue;
                    }
                    hit = Some((index, part));
                    break;
                }
            }
        }
        let policy = if self.kind == Kind::Section {
            RectangleSelectionPolicy::Section
        } else {
            RectangleSelectionPolicy::Range
        };
        let intent = policy.intent(modifiers);
        let time_range = if self.kind == Kind::Section {
            let Some(time_range) = crate::section_input::routes_to_time_range(
                hit.is_some_and(|(_, part)| part != RectanglePart::Body),
                double && hit.is_none(),
                y,
                f32::from(bounds.top()),
                f32::from(bounds.size.height),
                14.0,
            ) else {
                return false;
            };
            time_range
        } else {
            false
        };
        if self.kind == Kind::Section
            && intent != RectangleSelection::Replace
            && let Some((index, _)) = hit
        {
            let Some(fixture) = self.section_fixture.as_ref() else {
                return false;
            };
            let selection = crate::section_selection::extend(
                std::mem::take(&mut self.selected),
                self.selection_primary,
                self.selection_anchor,
                index,
                intent == RectangleSelection::Toggle,
                |anchor, clicked| {
                    crate::section_selection::range(
                        fixture
                            .project()
                            .arrangement_sections()
                            .iter()
                            .enumerate()
                            .map(|(index, section)| (index, section.start())),
                        anchor,
                        clicked,
                    )
                },
            );
            self.selected = selection.members;
            self.selection_primary = selection.selected;
            self.selection_anchor = Some(selection.anchor);
            self.status = "selected";
            self.focus.focus(window);
            cx.notify();
            return true;
        }
        if time_range {
            if !x.is_finite()
                || !left.is_finite()
                || !width.is_finite()
                || width <= 0.0
                || x < left
                || x >= left + width
            {
                return false;
            }
            let anchor = ((x - left) / width).clamp(0.0, 1.0);
            self.capture =
                RectangleCapture::begin(Target::SectionRange, RectanglePart::Body, x, true);
            self.range_before = self.range;
            self.range = Some([anchor, anchor]);
            self.selected.clear();
            self.selection_primary = None;
            self.status = "held range preview";
            self.focus.focus(window);
            cx.notify();
            return true;
        }
        let Some((index, part)) = hit else {
            if self.kind != Kind::Section
                || !double
                || !x.is_finite()
                || !width.is_finite()
                || width <= 0.0
                || x < left
                || x > left + width
            {
                return false;
            }
            let Some(fixture) = self.section_fixture.as_ref() else {
                return false;
            };
            let anchor = match fixture.snapped_beat(x, left, width, edit.invert_snap) {
                Ok(anchor) => anchor,
                Err(error) => {
                    self.status = error.0;
                    cx.notify();
                    return true;
                }
            };
            self.section_paint = Some(crate::section_appearance::new_marker(fixture.project()));
            self.section_anchor_beat = Some(anchor);
            self.capture =
                RectangleCapture::begin(Target::SectionPaint, RectanglePart::Body, x, true);
            self.section_last_pointer = Some((x, y));
            self.selected.clear();
            self.selection_primary = None;
            self.selection_anchor = None;
            self.focus.focus(window);
            self.change(x, y, modifiers, cx);
            return true;
        };
        match intent {
            RectangleSelection::Toggle => {
                if self.selected.contains(&index) {
                    self.selected.retain(|id| *id != index);
                } else {
                    self.selected.push(index);
                }
            }
            RectangleSelection::Range { additive } => {
                if !additive {
                    self.selected.clear();
                }
                for id in 0..self.canonical.len() {
                    if !self.selected.contains(&id) {
                        self.selected.push(id);
                    }
                }
            }
            RectangleSelection::Replace => {
                self.selected.clear();
                self.selected.push(index);
            }
        }
        if self.kind == Kind::Section {
            self.selection_primary = Some(index);
            self.selection_anchor = Some(index);
        }
        self.section_anchor_beat = if self.kind == Kind::Section && part == RectanglePart::Body {
            let Some(fixture) = self.section_fixture.as_ref() else {
                return false;
            };
            match fixture.snapped_beat(x, left, width, edit.invert_snap) {
                Ok(anchor) => Some(anchor),
                Err(error) => {
                    self.status = error.0;
                    cx.notify();
                    return true;
                }
            }
        } else {
            None
        };
        self.capture = RectangleCapture::begin(Target::Rectangle(index), part, x, true);
        self.section_marker_latched = self.kind == Kind::Section && edit.marker_only;
        self.section_gap_appearance = None;
        self.section_feedback = None;
        self.section_last_pointer = (self.kind == Kind::Section).then_some((x, y));
        self.preview = self.canonical.clone();
        if let Some(fixture) = self.section_fixture.as_ref() {
            self.section_preview = fixture.project().arrangement_sections().to_vec();
        }
        self.status = "held preview";
        self.focus.focus(window);
        cx.notify();
        true
    }
    fn change(&mut self, x: f32, y: f32, modifiers: Modifiers, cx: &mut Context<Self>) -> bool {
        let edit = crate::section_input::EditModifiers::from(modifiers);
        let section_rectangle = self.kind == Kind::Section
            && self.capture.is_some_and(|capture| {
                matches!(capture.key, Target::Rectangle(_) | Target::SectionPaint)
            });
        let x = if section_rectangle {
            let Some(bounds) = self.bounds else {
                return true;
            };
            let (left, right, top, bottom) = (
                f32::from(bounds.left()),
                f32::from(bounds.right()),
                f32::from(bounds.top()),
                f32::from(bounds.bottom()),
            );
            if x.is_finite() && y.is_finite() && left <= x && x <= right && top <= y && y <= bottom
            {
                self.section_last_pointer = Some((x, y));
            }
            let Some((last_x, _)) = self.section_last_pointer else {
                return true;
            };
            last_x
        } else {
            x
        };
        let Some(capture) = self.capture.as_mut() else {
            return false;
        };
        let Some(intent) = capture.update(x) else {
            return true;
        };
        let Some(bounds) = self.bounds else {
            return true;
        };
        let width = f32::from(bounds.size.width);
        if !width.is_finite() || width <= 0.0 {
            return true;
        }
        let delta = intent.delta_x / width;
        if intent.key == Target::SectionRange {
            let left = f32::from(bounds.left());
            let anchor = ((intent.origin_x - left) / width).clamp(0.0, 1.0);
            let end = ((intent.pointer_x - left) / width).clamp(0.0, 1.0);
            self.range = Some([anchor.min(end), anchor.max(end)]);
            cx.notify();
            return true;
        }
        if intent.key == Target::PaintRange {
            let left = f32::from(bounds.left());
            let anchor = ((intent.origin_x - left) / width).clamp(0.0, 1.0);
            let end = ((intent.pointer_x - left) / width).clamp(0.0, 1.0);
            self.preview[0] = [anchor.min(end), anchor.max(end)];
            cx.notify();
            return true;
        }
        if intent.key == Target::SectionPaint {
            self.section_command = None;
            self.section_feedback = None;
            self.section_error = None;
            self.preview = self.canonical.clone();
            if let (Some(fixture), Some((name, color))) =
                (&self.section_fixture, &self.section_paint)
            {
                self.section_preview = fixture.project().arrangement_sections().to_vec();
                let left = f32::from(bounds.left());
                let plan = self
                    .section_anchor_beat
                    .ok_or(hex_project::Invalid("section grid anchor unavailable"))
                    .and_then(|anchor| {
                        fixture
                            .snapped_beat(intent.pointer_x, left, width, edit.invert_snap)
                            .and_then(|position| {
                                fixture
                                    .paint_point_end(anchor, width, edit.invert_snap)
                                    .and_then(|end| {
                                        fixture.plan_paint(anchor, position, end, name, *color)
                                    })
                            })
                    });
                match plan {
                    Ok(plan) => {
                        self.preview = plan.display;
                        self.section_preview = plan.sections;
                        self.section_feedback = plan.feedback;
                        self.section_command = Some(plan.command);
                    }
                    Err(error) => {
                        self.section_error = Some(error.0);
                        self.status = error.0;
                    }
                }
            }
            cx.notify();
            return true;
        }
        let Target::Rectangle(index) = intent.key else {
            return true;
        };
        if self.kind == Kind::Section {
            if edit.marker_only {
                self.section_marker_latched = true;
            }
            self.section_command = None;
            self.section_feedback = None;
            self.section_error = None;
            self.preview = self.canonical.clone();
            if let Some(fixture) = self.section_fixture.as_ref() {
                self.section_preview = fixture.project().arrangement_sections().to_vec();
            }
            if (intent.pointer_x - intent.origin_x).abs() >= 4.0 {
                let left = f32::from(bounds.left());
                if let Some(fixture) = self.section_fixture.as_ref()
                    && let Some(section) = fixture.project().arrangement_sections().get(index)
                {
                    let plan = if intent.part == RectanglePart::Body && self.section_marker_latched
                    {
                        self.section_anchor_beat
                            .ok_or(hex_project::Invalid("section grid anchor unavailable"))
                            .and_then(|anchor| {
                                fixture
                                    .snapped_beat(intent.pointer_x, left, width, edit.invert_snap)
                                    .and_then(|beat| {
                                        fixture.plan_marker_move(section.id(), anchor, beat)
                                    })
                            })
                    } else if intent.part == RectanglePart::Body {
                        fixture
                            .snapped_beat(intent.pointer_x, left, width, true)
                            .and_then(|raw| {
                                let snapped = fixture.snapped_beat(
                                    intent.pointer_x,
                                    left,
                                    width,
                                    edit.invert_snap,
                                )?;
                                fixture.plan_drag(
                                    section.id(),
                                    raw,
                                    snapped,
                                    edit.copy_content,
                                    width,
                                    &mut self.section_gap_appearance,
                                )
                            })
                    } else {
                        fixture
                            .snapped_beat(intent.pointer_x, left, width, edit.invert_snap)
                            .and_then(|beat| {
                                fixture.plan_at_beat(
                                    section.id(),
                                    intent.part,
                                    beat,
                                    edit.copy_content,
                                    self.section_marker_latched,
                                )
                            })
                    };
                    match plan {
                        Ok(Some(plan)) => {
                            self.preview = plan.display;
                            self.section_preview = plan.sections;
                            self.section_feedback = plan.feedback;
                            self.section_command = Some(plan.command);
                        }
                        Ok(None) => {}
                        Err(error) => {
                            self.section_error = Some(error.0);
                            self.status = error.0;
                        }
                    }
                }
            }
            cx.notify();
            return true;
        }
        let [start, end] = self.canonical[index];
        let minimum = (1.0 / width).min(end - start);
        self.preview = self.canonical.clone();
        match intent.part {
            RectanglePart::Body => {
                let delta = delta.clamp(-start, 1.0 - end);
                self.preview[index] = [start + delta, end + delta];
            }
            RectanglePart::Leading => {
                self.preview[index][0] = (start + delta).clamp(0.0, end - minimum)
            }
            RectanglePart::Trailing => {
                self.preview[index][1] = (end + delta).clamp(start + minimum, 1.0)
            }
            RectanglePart::Seam => {
                let edge = (end + delta).clamp(start + minimum, self.canonical[1][1] - minimum);
                self.preview[0][1] = edge;
                self.preview[1][0] = edge;
            }
        }
        cx.notify();
        true
    }
    fn section_hover(&self) -> crate::section_surface::Hover {
        let mut hover = crate::section_surface::Hover::default();
        let (Some(capture), Some(fixture), Some(bounds)) =
            (self.capture, self.section_fixture.as_ref(), self.bounds)
        else {
            return hover;
        };
        if let Target::Rectangle(index) = capture.key {
            let sections = fixture.project().arrangement_sections();
            hover.held[0] = sections.get(index).map(ArrangementSection::id);
            if capture.part == RectanglePart::Seam && !self.section_marker_latched {
                hover.held[1] = index
                    .checked_add(1)
                    .and_then(|next| sections.get(next))
                    .map(ArrangementSection::id);
            }
        }
        if let Some((x, _)) = self.section_last_pointer {
            hover.pointer = fixture
                .snapped_beat(
                    x,
                    f32::from(bounds.left()),
                    f32::from(bounds.size.width),
                    true,
                )
                .ok();
        }
        hover
    }
    fn finish(&mut self, x: f32, y: f32, modifiers: Modifiers, cx: &mut Context<Self>) -> bool {
        if self.capture.is_none() {
            return false;
        }
        self.change(x, y, modifiers, cx);
        let x = if self.kind == Kind::Section
            && self.capture.is_some_and(|capture| {
                matches!(capture.key, Target::Rectangle(_) | Target::SectionPaint)
            }) {
            self.section_last_pointer.map_or(x, |(last_x, _)| last_x)
        } else {
            x
        };
        self.section_released_hover = self.section_hover();
        self.section_last_pointer = None;
        self.section_anchor_beat = None;
        self.section_paint = None;
        self.section_marker_latched = false;
        self.section_gap_appearance = None;
        if self
            .capture
            .is_some_and(|capture| capture.key == Target::SectionRange)
        {
            let completed = self
                .capture
                .take()
                .and_then(|capture| capture.finish(x))
                .is_some();
            if !completed {
                self.range = self.range_before;
            }
            self.range_before = None;
            self.status = if completed {
                "range selected"
            } else {
                "range canceled"
            };
            cx.notify();
            return true;
        }
        if self
            .capture
            .take()
            .and_then(|capture| capture.finish(x))
            .is_some()
            && if self.kind == Kind::Section {
                self.section_command.is_some()
            } else {
                self.preview != self.canonical
            }
        {
            if self.kind == Kind::Section {
                if let Some(command) = self.section_command.take() {
                    if !self.admit_section(
                        self.preview.clone(),
                        self.section_preview.clone(),
                        command,
                        1,
                    ) {
                        self.preview = self.canonical.clone();
                        self.status = "section edit refused";
                    }
                } else {
                    self.preview = self.canonical.clone();
                    self.status = "section edit refused";
                }
            } else {
                self.pending = Some(Pending::Local {
                    display: self.preview.clone(),
                });
                self.status = "pending · enter accepts · backspace refuses";
            }
        } else {
            self.preview = self.canonical.clone();
            self.status = self.section_error.take().unwrap_or("ready");
        }
        self.section_released_hover = crate::section_surface::Hover::default();
        cx.notify();
        true
    }
    /// The count includes this request and any earlier ordinary completions.
    fn admit_section(
        &mut self,
        display: Vec<[f32; 2]>,
        sections: Vec<ArrangementSection>,
        command: Command,
        pending_count: usize,
    ) -> bool {
        if self.kind != Kind::Section || self.pending.is_some() {
            return false;
        }
        let Some(source) = self
            .section_fixture
            .as_ref()
            .map(|fixture| fixture.project().clone())
        else {
            return false;
        };
        let mut receipt = crate::pending_edit::Receipt::new(self.generation);
        if !receipt.submitted(pending_count) {
            return false;
        }
        self.pending = Some(Pending::Section {
            display,
            sections,
            command: Box::new(command),
            source: Box::new(source),
            receipt,
            hover: std::mem::take(&mut self.section_released_hover),
            feedback: self.section_feedback.take(),
        });
        self.status = "pending · enter accepts · backspace refuses";
        true
    }

    /// Install a terminal command or a replacement project in the same receive
    /// that retires its frozen preview. Earlier FIFO completions retain custody.
    fn receive_section(
        &mut self,
        generation: u64,
        replaced: bool,
        request_complete: bool,
        accept: bool,
        replacement: Option<section_fixture::Fixture>,
        cx: &mut Context<Self>,
    ) {
        if (replaced || generation != self.generation) && replacement.is_none() {
            self.status = "section replacement unavailable";
            cx.notify();
            return;
        }
        let Some(Pending::Section { receipt, .. }) = self.pending.as_mut() else {
            return;
        };
        let disposition = receipt.observe(generation, replaced, request_complete);
        if disposition == crate::pending_edit::Disposition::Waiting {
            return;
        }
        // Selection indices belong to the source captured when this edit was
        // admitted, even if the installed project has since changed order.
        let Some(Pending::Section { source, .. }) = self.pending.as_ref() else {
            return;
        };
        let source_sections = source.arrangement_sections();
        let selected_ids: Vec<_> = self
            .selected
            .iter()
            .filter_map(|index| source_sections.get(*index).map(|section| section.id()))
            .collect();
        let selected_id = |index: Option<usize>| {
            index
                .and_then(|index| source_sections.get(index))
                .map(|section| section.id())
        };
        let primary_id = selected_id(self.selection_primary);
        let anchor_id = selected_id(self.selection_anchor);
        if let Some(replacement) = replacement {
            self.section_fixture = Some(replacement);
            self.selected.clear();
            self.selection_primary = None;
            self.selection_anchor = None;
        }
        let pending_edit = match self.pending.as_ref() {
            Some(Pending::Section {
                command, source, ..
            }) if disposition == crate::pending_edit::Disposition::Terminal && accept => {
                Some(((**command).clone(), source.as_ref()))
            }
            _ => None,
        };
        let mut commit_error = None;
        let mut created_copy = None;
        if let Some(fixture) = self.section_fixture.as_mut() {
            if let Some((command, source)) = pending_edit {
                if fixture.project() != source {
                    commit_error = Some("section project changed");
                } else {
                    let select_copy = matches!(command, Command::DuplicateArrangementSectionAt { section, .. } | Command::AppendArrangementSectionCopy { section, .. } | Command::ReplaceArrangementSection { source: section, copy: true, .. } if Some(section) == primary_id);
                    match fixture.commit(command) {
                        Ok(hex_project::Created::ArrangementSection(id)) if select_copy => {
                            created_copy = Some(id)
                        }
                        Ok(_) => {}
                        Err(error) => commit_error = Some(error.0),
                    }
                }
            }
            let sections = fixture.project().arrangement_sections();
            if disposition != crate::pending_edit::Disposition::Invalidated {
                self.selected = selected_ids
                    .iter()
                    .filter_map(|id| sections.iter().position(|section| section.id() == *id))
                    .collect();
                self.selection_primary = primary_id
                    .and_then(|id| sections.iter().position(|section| section.id() == id));
                self.selection_anchor =
                    anchor_id.and_then(|id| sections.iter().position(|section| section.id() == id));
                if let Some(index) = created_copy
                    .and_then(|id| sections.iter().position(|section| section.id() == id))
                {
                    self.selected = vec![index];
                    self.selection_primary = Some(index);
                    self.selection_anchor = Some(index);
                }
            }
            self.canonical = fixture.display();
            self.section_preview = sections.to_vec();
        }
        let display = self.pending.as_ref().map(Pending::display);
        self.pending = None;
        self.section_command = None;
        self.section_feedback = None;
        self.generation = generation;
        self.preview = self.canonical.clone();
        self.status = if let Some(error) = commit_error {
            error
        } else {
            match disposition {
                crate::pending_edit::Disposition::Invalidated => "source replaced",
                _ if accept && display.as_ref() == Some(&self.canonical) => "accepted",
                _ => "refused",
            }
        };
        cx.notify();
    }

    fn settle(&mut self, accept: bool, cx: &mut Context<Self>) {
        if matches!(self.pending, Some(Pending::Section { .. })) {
            self.receive_section(self.generation, false, true, accept, None, cx);
        } else if let Some(Pending::Local { display }) = self.pending.take() {
            if accept {
                self.canonical = display;
            }
            self.preview = self.canonical.clone();
            self.status = if accept { "accepted" } else { "refused" };
            cx.notify();
        }
    }

    fn section_selected(&self, painted: &ArrangementSection) -> bool {
        let project = match self.pending.as_ref() {
            Some(Pending::Section { source, .. }) => Some(source.as_ref()),
            _ => self
                .section_fixture
                .as_ref()
                .map(|fixture| fixture.project()),
        };
        let command = match self.pending.as_ref() {
            Some(Pending::Section { command, .. }) => Some(command.as_ref()),
            _ => self.section_command.as_ref(),
        };
        project.is_some_and(|project| {
            if let Some(Command::DuplicateArrangementSectionAt { section, .. } | Command::AppendArrangementSectionCopy { section, .. } | Command::ReplaceArrangementSection { source: section, copy: true, .. }) = command {
                return self
                    .selection_primary
                    .and_then(|index| project.arrangement_sections().get(index))
                    .is_some_and(|selected| selected.id() == *section)
                    && project.arrangement_section(painted.id()).is_none()
                    && !matches!(command, Some(Command::AppendArrangementSectionCopy { start, .. }) if painted.start() != *start);
            }
            self.selected.iter().any(|index| {
                project
                    .arrangement_sections()
                    .get(*index)
                    .is_some_and(|section| section.id() == painted.id())
            })
        })
    }
}
impl Focusable for RectangleDemo {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Render for RectangleDemo {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.blur.is_none() {
            let focus = self.focus.clone();
            self.blur = Some(cx.on_blur(&focus, window, |view, _, cx| view.cancel(cx)));
        }
        let style = theme::control_style();
        let display = self
            .pending
            .as_ref()
            .map(Pending::display)
            .unwrap_or_else(|| {
                if self.capture.is_some() {
                    self.preview.clone()
                } else {
                    self.canonical.clone()
                }
            });
        let section_paint = match self.pending.as_ref() {
            Some(Pending::Section { sections, .. }) => sections.clone(),
            _ if self.capture.is_some() => self.section_preview.clone(),
            _ => self
                .section_fixture
                .as_ref()
                .map_or_else(Vec::new, |fixture| {
                    fixture.project().arrangement_sections().to_vec()
                }),
        };
        let width = self
            .bounds
            .map_or(320.0, |bounds| f32::from(bounds.size.width));
        let mut area = div()
            .relative()
            .w_full()
            .h(px(72.0))
            .bg(rgb(style.surface))
            .debug_selector({
                let id = self.id.clone();
                move || format!("{id}-area")
            });
        let (section_hover, section_source) = match self.pending.as_ref() {
            Some(Pending::Section { hover, source, .. }) => (*hover, Some(source.as_ref())),
            _ => (
                self.section_hover(),
                self.section_fixture
                    .as_ref()
                    .map(|fixture| fixture.project()),
            ),
        };
        for (index, [start, end]) in display.iter().copied().enumerate() {
            if self.kind == Kind::TimeRange && index != 0 {
                continue;
            }
            let selected = if self.kind == Kind::Section {
                section_paint
                    .get(index)
                    .is_some_and(|painted| self.section_selected(painted))
            } else {
                self.selected.contains(&index)
            };
            let pixel_width = (end - start) * width;
            let identity = section_paint
                .get(index)
                .map_or(hex_project::ElementColor::Cyan, ArrangementSection::color);
            let hovered = section_paint.get(index).is_some_and(|section| {
                let id = section_source.and_then(|project| {
                    project
                        .arrangement_sections()
                        .iter()
                        .find(|candidate| candidate.id() == section.id())
                        .map(ArrangementSection::id)
                });
                section_hover.contains(id, section.start(), section.end())
            });
            let held_marker = self.section_marker_latched
                || matches!(
                    self.pending.as_ref(),
                    Some(Pending::Section { command, .. }) if matches!(command.as_ref(),
                        Command::MoveArrangementSectionMarker { .. }
                        | Command::ResizeArrangementSectionMarker { .. })
                );
            let hover_marker = self.pending.is_none()
                && self.capture.is_none()
                && crate::section_input::EditModifiers::from(window.modifiers()).marker_only
                && self.bounds.is_some_and(|bounds| {
                    let pointer = window.mouse_position();
                    bounds.contains(&pointer)
                        && pointer.x >= bounds.left() + px(start * width)
                        && pointer.x < bounds.left() + px(end * width)
                });
            let marker_only =
                self.kind == Kind::Section && ((held_marker && hovered) || hover_marker);
            let surface = if self.kind == Kind::Section {
                crate::section_surface::surface(
                    start,
                    end - start,
                    pixel_width,
                    crate::section_surface::CHAMFER,
                    identity,
                    selected,
                    hovered,
                )
                .cursor(RectanglePart::Body.cursor())
                .children(marker_only.then(|| {
                    crate::section_surface::marker_only_overlay(
                        identity,
                        crate::section_surface::CHAMFER,
                        selected,
                    )
                }))
                .children((self.section_focus && selected).then(|| {
                    crate::section_surface::focus::overlay(
                        identity,
                        crate::section_surface::CHAMFER,
                    )
                    .debug_selector({
                        let id = self.id.clone();
                        move || format!("{id}-section-focus-{index}")
                    })
                }))
                .child(
                    crate::section_surface::header(identity, selected, hovered, 14.0)
                        .when(marker_only, |header| {
                            header.text_color(rgb(theme::color(theme::PaletteRole::Text)))
                        })
                        .debug_selector({
                            let id = self.id.clone();
                            move || format!("{id}-section-header-{index}")
                        })
                        .child(theme::text({
                            let name = section_paint.get(index).map_or_else(
                                || "section".to_owned(),
                                |section| section.name().to_owned(),
                            );
                            if marker_only {
                                format!("◇ {name}")
                            } else {
                                name
                            }
                        })),
                )
            } else {
                rectangle_surface(start, end - start, pixel_width)
                    .bg(rgb(if selected { style.accent } else { style.border }))
                    .border_color(rgb(if self.focus.is_focused(window) {
                        style.text
                    } else {
                        style.border
                    }))
                    .cursor(gpui::CursorStyle::Arrow)
                    .text_color(rgb(if selected {
                        style.accent_text
                    } else {
                        style.text
                    }))
                    .child(theme::text(if self.kind == Kind::Clip {
                        "clip"
                    } else {
                        "time range"
                    }))
            };
            let mut rect = surface
                .id(("rectangle", index))
                .debug_selector({
                    let id = self.id.clone();
                    move || format!("{id}-rectangle-{index}")
                })
                .opacity(if self.enabled { 1.0 } else { 0.4 });
            let border = hex_widgets::rectangle_border_width(pixel_width);
            let handle = self.edges().handle_width(pixel_width);
            if self.kind == Kind::TimeRange
                && let Some(zones) = RangeSelectionZones::new(72.0, 0.0)
            {
                rect = rect.child(rectangle_body(-1.0, zones.handle_height()));
            }
            if self.kind == Kind::Clip {
                rect = rect.child(
                    rectangle_body(-border, 14.0)
                        .left(px(handle - border))
                        .right(px(handle - border)),
                );
            }
            if self.kind != Kind::TimeRange {
                for part in [RectanglePart::Leading, RectanglePart::Trailing] {
                    rect = rect.child(rectangle_edge(part, handle, -border));
                }
            }
            area = area.child(rect);
        }
        if self.kind == Kind::Section
            && let Some([start, end]) = self.range
            && end > start
        {
            area = area.child(
                rectangle_surface(start, end - start, width * (end - start))
                    .debug_selector(|| "section-time-range".into())
                    .bg(rgb(style.accent))
                    .opacity(0.25),
            );
        }
        if self.kind == Kind::Section {
            for (index, (pair, sections)) in
                display.windows(2).zip(section_paint.windows(2)).enumerate()
            {
                if sections[0].end() == sections[1].start() {
                    area = area.child(
                        rectangle_edge(RectanglePart::Seam, 2.0, 0.0)
                            .debug_selector({
                                let id = self.id.clone();
                                move || format!("{id}-seam-{index}")
                            })
                            .left(gpui::relative(pair[0][1]))
                            .ml(px(-1.0)),
                    );
                }
            }
        }
        let (feedback, project) = match self.pending.as_ref() {
            Some(Pending::Section {
                feedback, source, ..
            }) => (*feedback, Some(source.as_ref())),
            _ if self.capture.is_some() => (
                self.section_feedback,
                self.section_fixture.as_ref().map(|f| f.project()),
            ),
            _ => (None, None),
        };
        if let (Some(feedback), Some(project)) = (feedback, project) {
            match feedback {
                crate::section_reorder::DropFeedback::Insert(at) => {
                    area = area.child(
                        crate::section_surface::insertion_indicator(section_fixture::position_in(
                            project, at,
                        ))
                        .debug_selector(|| "sandbox-section-drop-insertion".into()),
                    );
                }
                crate::section_reorder::DropFeedback::Replace { start, end, .. } => {
                    let left = section_fixture::position_in(project, start);
                    area = area.child(
                        crate::section_surface::replacement_indicator(
                            left,
                            section_fixture::position_in(project, end) - left,
                        )
                        .debug_selector(|| "sandbox-section-drop-replace".into()),
                    );
                }
            }
        }
        let entity = cx.entity();
        area = area.child(
            gpui::canvas(
                |_, _, _| (),
                move |bounds, _, window, cx| {
                    entity.update(cx, |view, cx| {
                        if view.bounds != Some(bounds) {
                            view.bounds = Some(bounds);
                            cx.notify();
                        }
                    });
                    let down = entity.clone();
                    window.on_mouse_event(
                        move |event: &gpui::MouseDownEvent, phase, window, cx| {
                            if phase.capture()
                                && event.button == MouseButton::Left
                                && down.update(cx, |view, cx| {
                                    view.begin(
                                        event.position.x.into(),
                                        event.position.y.into(),
                                        event.click_count >= 2,
                                        event.modifiers,
                                        window,
                                        cx,
                                    )
                                })
                            {
                                cx.stop_propagation();
                            }
                        },
                    );
                    let moved = entity.clone();
                    window.on_mouse_event(move |event: &gpui::MouseMoveEvent, phase, _, cx| {
                        if !phase.capture() {
                            return;
                        }
                        let held = moved.update(cx, |view, cx| {
                            if event.pressed_button == Some(MouseButton::Left) {
                                view.change(
                                    event.position.x.into(),
                                    event.position.y.into(),
                                    event.modifiers,
                                    cx,
                                )
                            } else {
                                let held = view.capture.is_some();
                                view.cancel(cx);
                                held
                            }
                        });
                        if held {
                            cx.stop_propagation();
                        }
                    });
                    let up = entity.clone();
                    window.on_mouse_event(move |event: &gpui::MouseUpEvent, phase, _, cx| {
                        if phase.capture()
                            && event.button == MouseButton::Left
                            && up.update(cx, |view, cx| {
                                view.finish(
                                    event.position.x.into(),
                                    event.position.y.into(),
                                    event.modifiers,
                                    cx,
                                )
                            })
                        {
                            cx.stop_propagation();
                        }
                    });
                },
            )
            .absolute()
            .inset_0(),
        );
        div()
            .id(gpui::SharedString::from(self.id.clone()))
            .track_focus(&self.focus)
            .flex()
            .flex_col()
            .gap_2()
            .w_full()
            .on_modifiers_changed(cx.listener(
                |view, event: &gpui::ModifiersChangedEvent, window, cx| {
                    if view.kind == Kind::Section {
                        cx.notify();
                    }
                    if view.kind == Kind::Section && view.capture.is_some() {
                        let position = window.mouse_position();
                        view.change(position.x.into(), position.y.into(), event.modifiers, cx);
                    }
                },
            ))
            .on_key_down(cx.listener(|view, event: &gpui::KeyDownEvent, _, cx| {
                match event.keystroke.key.as_str() {
                    "q" if view.kind == Kind::Section && !event.keystroke.modifiers.modified() => {
                        if !event.is_held {
                            view.section_focus = !view.section_focus;
                            cx.notify();
                        }
                    }
                    "escape" => view.cancel(cx),
                    "enter" => view.settle(true, cx),
                    "backspace" => view.settle(false, cx),
                    _ => return,
                }
                cx.stop_propagation();
            }))
            .child(area)
            .child(theme::text(self.status))
            .children(
                (self.kind == Kind::Section).then(|| theme::text("q toggles section focus paint")),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        div()
                            .id("accept")
                            .child(hex_widgets::action(
                                theme::text("accept pending"),
                                theme::action_style(),
                            ))
                            .on_click(cx.listener(|view, _, _, cx| view.settle(true, cx))),
                    )
                    .child(
                        div()
                            .id("refuse")
                            .child(hex_widgets::action(
                                theme::text("refuse pending"),
                                theme::action_style(),
                            ))
                            .on_click(cx.listener(|view, _, _, cx| view.settle(false, cx))),
                    ),
            )
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests {
    use super::*;
    use gpui::{TestAppContext, VisualTestContext, point};

    // The shared chamfer painter fills canvas fragments behind a transparent
    // header. Observe the completed scene at its center, not a nonexistent
    // full-bounds header quad.
    fn painted_surface(
        visual: &mut VisualTestContext,
        selector: &'static str,
    ) -> Option<gpui::Background> {
        let bounds = visual.debug_bounds(selector)?;
        let center = visual.update(|window, _| bounds.scale(window.scale_factor()).center());
        visual
            .rendered_filled_quads(selector)
            .into_iter()
            .rev()
            .find(|(quad, _)| quad.contains(&center))
            .map(|(_, background)| background)
    }
    #[gpui::test]
    fn q_previews_shared_section_focus_without_editing_the_project(cx: &mut TestAppContext) {
        let handle =
            cx.add_window(|_, cx| RectangleDemo::new(Kind::Section, "focus-fixture".into(), cx));
        let view = handle.root(cx).unwrap();
        let mut visual = VisualTestContext::from_window(*handle, cx);
        visual.update(|window, cx| {
            view.update(cx, |v, cx| {
                v.selected = vec![0];
                v.focus.focus(window);
                cx.notify();
            })
        });
        visual.run_until_parked();
        let before = view.read_with(cx, |v, _| {
            v.section_fixture.as_ref().unwrap().project().snapshot()
        });
        assert!(
            visual
                .debug_bounds("focus-fixture-section-focus-0")
                .is_none()
        );
        visual.simulate_keystrokes("q");
        visual.run_until_parked();
        assert!(
            visual
                .debug_bounds("focus-fixture-section-focus-0")
                .is_some()
        );
        assert!(
            visual
                .debug_bounds("focus-fixture-section-focus-1")
                .is_none()
        );
        visual.simulate_keystrokes("q");
        visual.run_until_parked();
        assert!(
            visual
                .debug_bounds("focus-fixture-section-focus-0")
                .is_none()
        );
        view.read_with(cx, |v, _| {
            assert_eq!(
                v.section_fixture.as_ref().unwrap().project().snapshot(),
                before
            )
        });
    }

    #[gpui::test]
    fn time_range_body_repaints_while_upper_rail_moves_and_pending_survives_cancel(
        cx: &mut TestAppContext,
    ) {
        let handle =
            cx.add_window(|_, cx| RectangleDemo::new(Kind::TimeRange, "range-fixture".into(), cx));
        let view = handle.root(cx).unwrap();
        let mut visual = VisualTestContext::from_window(*handle, cx);
        visual.update(|window, _| window.activate_window());
        visual.run_until_parked();
        let area = visual.debug_bounds("range-fixture-area").unwrap();
        let at = |x, y| {
            point(
                area.left() + area.size.width * x,
                area.top() + area.size.height * y,
            )
        };
        let baseline = view.read_with(cx, |view, _| view.canonical.clone());
        for (down, up, modifiers, expected) in [
            (
                at(0.4, 0.75),
                at(0.2, 0.75),
                Modifiers::default(),
                [0.2, 0.4],
            ),
            (at(0.3, 0.1), at(0.4, 0.1), Modifiers::default(), [0.2, 0.6]),
            (
                at(0.3, 0.1),
                at(0.6, 0.1),
                Modifiers {
                    shift: true,
                    ..Modifiers::default()
                },
                [0.3, 0.6],
            ),
        ] {
            visual.simulate_mouse_down(down, MouseButton::Left, modifiers);
            visual.simulate_mouse_move(up, MouseButton::Left, modifiers);
            visual.simulate_mouse_up(up, MouseButton::Left, modifiers);
            for _ in 0..3 {
                view.update(cx, |_, cx| cx.notify());
                visual.run_until_parked();
                view.read_with(cx, |view, _| {
                    assert_eq!(view.canonical, baseline);
                    let actual = view.pending.as_ref().map(Pending::display).unwrap()[0];
                    for i in 0..2 {
                        assert!(
                            (actual[i] - expected[i]).abs() < 0.001,
                            "{actual:?} != {expected:?}"
                        );
                    }
                });
            }
            visual.simulate_keystrokes("escape");
            view.update(cx, |view, cx| view.set_enabled(false, cx));
            view.read_with(cx, |view, _| assert!(view.pending.is_some()));
            view.update(cx, |view, cx| {
                view.settle(false, cx);
                view.set_enabled(true, cx);
            });
            visual.run_until_parked();
        }
        visual.simulate_mouse_down(at(0.4, 0.75), MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(at(0.2, 0.75), MouseButton::Left, Modifiers::default());
        visual.simulate_keystrokes("escape");
        visual.run_until_parked();
        view.read_with(cx, |view, _| {
            assert!(view.capture.is_none());
            assert!(view.pending.is_none());
            assert_eq!(view.preview, baseline);
        });
        view.update(cx, |view, cx| view.set_enabled(false, cx));
        visual.simulate_mouse_down(at(0.3, 0.1), MouseButton::Left, Modifiers::default());
        view.read_with(cx, |view, _| assert!(view.capture.is_none()));
    }
    #[gpui::test]
    fn section_lower_body_drag_never_moves_section_geometry(cx: &mut TestAppContext) {
        let handle =
            cx.add_window(|_, cx| RectangleDemo::new(Kind::Section, "section-body".into(), cx));
        let view = handle.root(cx).unwrap();
        let mut visual = VisualTestContext::from_window(*handle, cx);
        visual.update(|window, _| window.activate_window());
        visual.run_until_parked();
        let first = visual.debug_bounds("section-body-rectangle-0").unwrap();
        let at = point(first.center().x, first.top() + first.size.height * 0.75);
        let before = view.read_with(cx, |view, _| view.canonical.clone());
        visual.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(
            point(at.x + px(24.0), at.y),
            MouseButton::Left,
            Modifiers::default(),
        );
        view.read_with(cx, |view, _| {
            assert_eq!(view.canonical, before);
            assert_eq!(
                view.preview, before,
                "lower body must not preview a section move"
            );
        });
    }

    #[gpui::test]
    fn section_body_range_commits_and_cancel_restores_previous_range(cx: &mut TestAppContext) {
        let handle = cx
            .add_window(|_, cx| RectangleDemo::new(Kind::Section, "section-range-body".into(), cx));
        let view = handle.root(cx).unwrap();
        let mut visual = VisualTestContext::from_window(*handle, cx);
        visual.update(|window, _| window.activate_window());
        visual.run_until_parked();
        let area = visual.debug_bounds("section-range-body-area").unwrap();
        let at = |fraction: f32| {
            point(
                area.left() + area.size.width * fraction,
                area.top() + area.size.height * 0.75,
            )
        };
        let before = view.read_with(cx, |view, _| view.canonical.clone());
        visual.simulate_mouse_down(at(0.2), MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(at(0.35), MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(at(0.35), MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        let committed = view.read_with(cx, |view, _| {
            assert_eq!(view.canonical, before);
            assert_eq!(view.preview, before);
            assert!(view.pending.is_none());
            view.range.expect("committed time range")
        });
        assert!(committed[1] > committed[0]);
        assert!(visual.debug_bounds("section-time-range").is_some());
        visual.simulate_mouse_down(at(0.3), MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(at(0.4), MouseButton::Left, Modifiers::default());
        visual.simulate_keystrokes("escape");
        view.read_with(cx, |view, _| {
            assert_eq!(view.range, Some(committed));
            assert_eq!(view.canonical, before);
            assert!(view.capture.is_none());
        });
        visual.simulate_mouse_down(at(0.3), MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(at(0.4), MouseButton::Left, Modifiers::default());
        view.update(cx, |view, cx| {
            assert!(view.finish(f32::NAN, 0.0, Modifiers::default(), cx))
        });
        view.read_with(cx, |view, _| {
            assert_eq!(view.range, Some(committed));
            assert!(view.capture.is_none());
        });
    }

    #[gpui::test]
    fn section_edges_and_extended_selection_win_over_lower_body_range(cx: &mut TestAppContext) {
        let handle =
            cx.add_window(|_, cx| RectangleDemo::new(Kind::Section, "section-priority".into(), cx));
        let view = handle.root(cx).unwrap();
        let mut visual = VisualTestContext::from_window(*handle, cx);
        visual.update(|window, _| window.activate_window());
        visual.run_until_parked();
        let first = visual.debug_bounds("section-priority-rectangle-0").unwrap();
        let edge = point(
            first.left() + px(2.0),
            first.top() + first.size.height * 0.75,
        );
        visual.simulate_mouse_down(edge, MouseButton::Left, Modifiers::default());
        view.read_with(cx, |view, _| {
            let capture = view.capture.expect("edge capture");
            assert_eq!(capture.key, Target::Rectangle(0));
            assert_eq!(capture.part, RectanglePart::Leading);
            assert!(view.range.is_none());
        });
        visual.simulate_keystrokes("escape");
        let body = point(first.center().x, first.top() + first.size.height * 0.75);
        visual.simulate_mouse_down(
            body,
            MouseButton::Left,
            Modifiers {
                shift: true,
                ..Modifiers::default()
            },
        );
        view.read_with(cx, |view, _| {
            assert!(view.capture.is_none());
            assert!(view.range.is_none());
            assert_eq!(view.selected, [0]);
        });
    }

    #[gpui::test]
    fn shift_click_same_section_keeps_only_that_section_and_never_starts_drag(
        cx: &mut TestAppContext,
    ) {
        let handle =
            cx.add_window(|_, cx| RectangleDemo::new(Kind::Section, "section-shift".into(), cx));
        let view = handle.root(cx).unwrap();
        let mut visual = VisualTestContext::from_window(*handle, cx);
        visual.update(|window, _| window.activate_window());
        visual.run_until_parked();
        let first = visual.debug_bounds("section-shift-rectangle-0").unwrap();
        let at = point(first.center().x, first.top() + px(5.0));
        visual.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        let other_before = visual.rendered_background("section-shift-rectangle-1");
        visual.simulate_mouse_down(
            at,
            MouseButton::Left,
            Modifiers {
                shift: true,
                ..Modifiers::default()
            },
        );
        visual.run_until_parked();
        view.read_with(cx, |view, _| {
            assert!(
                view.capture.is_none(),
                "shift selection must not start a drag"
            )
        });
        assert_eq!(
            visual.rendered_background("section-shift-rectangle-1"),
            other_before,
            "same-section range must not select the other section"
        );
    }

    #[gpui::test]
    fn extended_section_selection_keeps_anchor_and_never_captures_a_drag(cx: &mut TestAppContext) {
        let handle =
            cx.add_window(|_, cx| RectangleDemo::new(Kind::Section, "section-range".into(), cx));
        let view = handle.root(cx).unwrap();
        let mut visual = VisualTestContext::from_window(*handle, cx);
        visual.update(|window, _| window.activate_window());
        visual.run_until_parked();
        let first = visual.debug_bounds("section-range-rectangle-0").unwrap();
        let second = visual.debug_bounds("section-range-rectangle-1").unwrap();
        let a = point(first.center().x, first.top() + px(5.0));
        let b = point(second.center().x, second.top() + px(5.0));
        let shift = Modifiers {
            shift: true,
            ..Modifiers::default()
        };
        let toggle = Modifiers {
            shift: true,
            platform: true,
            ..Modifiers::default()
        };
        visual.simulate_mouse_down(a, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(a, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_down(b, MouseButton::Left, shift);
        view.read_with(cx, |view, _| {
            assert_eq!(view.selected, [0, 1]);
            assert_eq!(view.selection_anchor, Some(0));
            assert!(view.capture.is_none());
        });
        visual.simulate_mouse_up(b, MouseButton::Left, shift);
        visual.simulate_mouse_down(a, MouseButton::Left, toggle);
        view.read_with(cx, |view, _| {
            assert_eq!(view.selected, [1]);
            assert_eq!(view.selection_primary, Some(1));
            assert_eq!(view.selection_anchor, Some(0));
            assert!(view.capture.is_none());
        });
        visual.simulate_mouse_up(a, MouseButton::Left, toggle);
        visual.simulate_mouse_down(b, MouseButton::Left, shift);
        view.read_with(cx, |view, _| {
            assert_eq!(view.selected, [1]);
            assert_eq!(view.selection_anchor, Some(1));
            assert!(view.capture.is_none());
        });
    }

    fn seam_plan(view: &RectangleDemo, position: f32) -> section_fixture::Plan {
        let fixture = view.section_fixture.as_ref().unwrap();
        let section = fixture.project().arrangement_sections()[0].id();
        fixture
            .plan(section, RectanglePart::Seam, position, false, false)
            .unwrap()
            .unwrap()
    }

    #[gpui::test]
    fn section_receipt_retains_released_paint_until_its_terminal_receive(cx: &mut TestAppContext) {
        let handle =
            cx.add_window(|_, cx| RectangleDemo::new(Kind::Section, "section-receipt".into(), cx));
        let view = handle.root(cx).unwrap();
        let mut visual = VisualTestContext::from_window(*handle, cx);
        let before = view.read_with(cx, |view, _| view.canonical.clone());
        let released = view.update(cx, |view, cx| {
            let plan = seam_plan(view, 0.55);
            let released = plan.display.clone();
            assert!(view.admit_section(plan.display, plan.sections, plan.command, 2));
            cx.notify();
            released
        });
        visual.run_until_parked();
        let frozen = visual.debug_bounds("section-receipt-rectangle-0").unwrap();
        view.update(cx, |view, cx| {
            view.receive_section(1, false, true, true, None, cx)
        });
        for _ in 0..3 {
            view.update(cx, |_, cx| cx.notify());
            visual.run_until_parked();
            assert_eq!(
                visual.debug_bounds("section-receipt-rectangle-0").unwrap(),
                frozen
            );
            view.read_with(cx, |view, _| {
                assert_eq!(
                    view.pending.as_ref().map(Pending::display),
                    Some(released.clone())
                );
                assert_eq!(view.canonical, before);
                assert_eq!(view.section_fixture.as_ref().unwrap().display(), before);
            });
        }
        view.update(cx, |view, cx| {
            view.receive_section(1, false, true, true, None, cx)
        });
        visual.run_until_parked();
        assert_eq!(
            visual.debug_bounds("section-receipt-rectangle-0").unwrap(),
            frozen
        );
        view.read_with(cx, |view, _| {
            assert!(view.pending.is_none());
            assert_eq!(view.canonical, released);
            assert_eq!(view.section_fixture.as_ref().unwrap().display(), released);
            assert_eq!(view.preview, released);
        });
    }

    #[gpui::test]
    fn shared_seam_highlight_stays_visible_through_pending_repaints(cx: &mut TestAppContext) {
        let handle = cx.add_window(|_, cx| {
            RectangleDemo::new(Kind::Section, "section-held-highlight".into(), cx)
        });
        let view = handle.root(cx).unwrap();
        let mut visual = VisualTestContext::from_window(*handle, cx);
        visual.update(|window, _| window.activate_window());
        visual.run_until_parked();
        let area = visual.debug_bounds("section-held-highlight-area").unwrap();
        let at = |x: f32| point(area.left() + area.size.width * x, area.top() + px(5.0));
        let outside = point(area.right() + px(50.0), area.top() - px(10.0));
        let baseline = painted_surface(&mut visual, "section-held-highlight-section-header-1");
        visual.simulate_mouse_down(at(0.5), MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(at(0.43), MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(outside, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(outside, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        view.read_with(cx, |view, _| assert!(view.pending.is_some()));
        for _ in 0..3 {
            view.update(cx, |_, cx| cx.notify());
            visual.run_until_parked();
            assert_ne!(
                painted_surface(&mut visual, "section-held-highlight-section-header-1"),
                baseline
            );
        }
        view.update(cx, |view, cx| {
            view.receive_section(1, false, true, false, None, cx)
        });
        visual.run_until_parked();
        assert_eq!(
            painted_surface(&mut visual, "section-held-highlight-section-header-1"),
            baseline
        );
    }

    #[gpui::test]
    fn section_receipt_refusal_and_source_replacement_install_canonical_state(
        cx: &mut TestAppContext,
    ) {
        let handle = cx.add_window(|_, cx| {
            RectangleDemo::new(Kind::Section, "section-receipt-reset".into(), cx)
        });
        let view = handle.root(cx).unwrap();
        let mut visual = VisualTestContext::from_window(*handle, cx);
        let before = view.read_with(cx, |view, _| view.canonical.clone());
        visual.run_until_parked();
        let original_bounds = visual
            .debug_bounds("section-receipt-reset-rectangle-0")
            .unwrap();
        view.update(cx, |view, cx| {
            let plan = seam_plan(view, 0.55);
            assert!(view.admit_section(plan.display, plan.sections, plan.command, 1));
            cx.notify();
        });
        visual.run_until_parked();
        let frozen = visual
            .debug_bounds("section-receipt-reset-rectangle-0")
            .unwrap();
        assert_ne!(frozen, original_bounds);
        view.update(cx, |view, cx| {
            view.receive_section(1, false, true, false, None, cx)
        });
        visual.run_until_parked();
        assert_eq!(
            visual
                .debug_bounds("section-receipt-reset-rectangle-0")
                .unwrap(),
            original_bounds
        );
        view.read_with(cx, |view, _| {
            assert!(view.pending.is_none());
            assert_eq!(view.preview, before);
            assert_eq!(view.section_fixture.as_ref().unwrap().display(), before);
        });
        view.update(cx, |view, cx| {
            let plan = seam_plan(view, 0.55);
            assert!(view.admit_section(plan.display, plan.sections, plan.command, 2));
            cx.notify();
        });
        visual.run_until_parked();
        assert_eq!(
            visual
                .debug_bounds("section-receipt-reset-rectangle-0")
                .unwrap(),
            frozen
        );
        let mut replacement = section_fixture::Fixture::new().unwrap();
        let id = replacement.project().arrangement_sections()[0].id();
        let replacement_plan = replacement
            .plan(id, RectanglePart::Seam, 0.6, false, false)
            .unwrap()
            .unwrap();
        replacement.commit(replacement_plan.command).unwrap();
        let replacement_display = replacement.display();
        view.update(cx, |view, cx| {
            view.receive_section(2, true, false, false, Some(replacement), cx)
        });
        visual.run_until_parked();
        assert_ne!(
            visual
                .debug_bounds("section-receipt-reset-rectangle-0")
                .unwrap(),
            frozen
        );
        view.read_with(cx, |view, _| {
            assert!(view.pending.is_none());
            assert_eq!(view.generation, 2);
            assert_eq!(view.canonical, replacement_display);
            assert_eq!(view.preview, replacement_display);
            assert_eq!(
                view.section_fixture.as_ref().unwrap().display(),
                replacement_display
            );
        });
    }

    #[gpui::test]
    fn section_seam_gesture_commits_real_project_only_after_terminal(cx: &mut TestAppContext) {
        let handle = cx
            .add_window(|_, cx| RectangleDemo::new(Kind::Section, "section-real-seam".into(), cx));
        let view = handle.root(cx).unwrap();
        let mut visual = VisualTestContext::from_window(*handle, cx);
        visual.update(|window, _| window.activate_window());
        visual.run_until_parked();
        let first = visual
            .debug_bounds("section-real-seam-rectangle-0")
            .unwrap();
        let down = point(first.right(), first.top() + px(5.0));
        let target = point(down.x + px(12.0), down.y);
        let before = view.read_with(cx, |view, _| {
            view.section_fixture.as_ref().unwrap().project().snapshot()
        });
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        let frozen = visual
            .debug_bounds("section-real-seam-rectangle-0")
            .unwrap();
        view.read_with(cx, |view, _| {
            assert!(view.pending.is_some());
            assert_eq!(
                view.section_fixture.as_ref().unwrap().project().snapshot(),
                before
            );
        });
        for _ in 0..3 {
            view.update(cx, |_, cx| cx.notify());
            visual.run_until_parked();
            assert_eq!(
                visual
                    .debug_bounds("section-real-seam-rectangle-0")
                    .unwrap(),
                frozen
            );
        }
        view.update(cx, |view, cx| view.settle(true, cx));
        visual.run_until_parked();
        assert_eq!(
            visual
                .debug_bounds("section-real-seam-rectangle-0")
                .unwrap(),
            frozen
        );
        view.read_with(cx, |view, _| {
            assert!(view.pending.is_none());
            assert_ne!(
                view.section_fixture.as_ref().unwrap().project().snapshot(),
                before
            );
            assert_eq!(
                view.canonical,
                view.section_fixture.as_ref().unwrap().display()
            );
        });
    }

    #[gpui::test]
    fn stale_section_terminal_refuses_without_overwriting_new_project(cx: &mut TestAppContext) {
        let handle =
            cx.add_window(|_, cx| RectangleDemo::new(Kind::Section, "section-stale".into(), cx));
        let view = handle.root(cx).unwrap();
        view.update(cx, |view, cx| {
            let pending = seam_plan(view, 0.55);
            assert!(view.admit_section(pending.display, pending.sections, pending.command, 1));
            let newer = seam_plan(view, 0.6);
            view.section_fixture
                .as_mut()
                .unwrap()
                .commit(newer.command)
                .unwrap();
            let current = view.section_fixture.as_ref().unwrap().project().snapshot();
            view.receive_section(1, false, true, true, None, cx);
            assert_eq!(
                view.section_fixture.as_ref().unwrap().project().snapshot(),
                current
            );
            assert_eq!(
                view.canonical,
                view.section_fixture.as_ref().unwrap().display()
            );
            assert!(view.pending.is_none());
            assert_eq!(view.status, "section project changed");
        });
    }

    #[gpui::test]
    fn stale_section_terminal_keeps_selection_identity_after_reorder(cx: &mut TestAppContext) {
        let handle = cx.add_window(|_, cx| {
            RectangleDemo::new(Kind::Section, "section-stale-selection".into(), cx)
        });
        let view = handle.root(cx).unwrap();
        view.update(cx, |view, cx| {
            let id = view
                .section_fixture
                .as_ref()
                .unwrap()
                .project()
                .arrangement_sections()[0]
                .id();
            view.selected = vec![0];
            view.selection_primary = Some(0);
            view.selection_anchor = Some(0);
            let pending = seam_plan(view, 0.55);
            assert!(view.admit_section(pending.display, pending.sections, pending.command, 1));
            let fixture = view.section_fixture.as_mut().unwrap();
            let newer = fixture
                .plan(id, RectanglePart::Body, 0.95, false, false)
                .unwrap()
                .unwrap();
            fixture.commit(newer.command).unwrap();
            let current = fixture.project().snapshot();
            assert_eq!(fixture.project().arrangement_sections()[1].id(), id);
            view.receive_section(1, false, true, true, None, cx);
            assert_eq!(
                view.section_fixture.as_ref().unwrap().project().snapshot(),
                current
            );
            assert_eq!(view.selected, [1]);
            assert_eq!(view.selection_primary, Some(1));
            assert_eq!(view.selection_anchor, Some(1));
            assert_eq!(
                view.canonical,
                view.section_fixture.as_ref().unwrap().display()
            );
            assert_eq!(view.preview, view.canonical);
            assert!(view.pending.is_none());
            assert_eq!(view.status, "section project changed");
        });
    }

    #[gpui::test]
    fn pending_repaints_keep_selection_identity_after_reorder(cx: &mut TestAppContext) {
        let handle = cx.add_window(|_, cx| {
            RectangleDemo::new(Kind::Section, "section-pending-selection".into(), cx)
        });
        let view = handle.root(cx).unwrap();
        let id = view.update(cx, |view, cx| {
            let id = view
                .section_fixture
                .as_ref()
                .unwrap()
                .project()
                .arrangement_sections()[0]
                .id();
            view.selected = vec![0];
            let pending = seam_plan(view, 0.55);
            assert!(view.admit_section(pending.display, pending.sections, pending.command, 1));
            let fixture = view.section_fixture.as_mut().unwrap();
            let newer = fixture
                .plan(id, RectanglePart::Body, 0.95, false, false)
                .unwrap()
                .unwrap();
            fixture.commit(newer.command).unwrap();
            cx.notify();
            id
        });
        let visual = VisualTestContext::from_window(*handle, cx);
        for _ in 0..3 {
            view.update(cx, |_, cx| cx.notify());
            visual.run_until_parked();
            view.read_with(cx, |view, _| {
                let Some(Pending::Section { sections, .. }) = view.pending.as_ref() else {
                    panic!("section edit should remain pending");
                };
                for section in sections {
                    assert_eq!(view.section_selected(section), section.id() == id);
                }
            });
        }
    }

    #[gpui::test]
    fn rounded_gap_does_not_draw_a_shared_seam(cx: &mut TestAppContext) {
        let handle = cx.add_window(|_, cx| {
            RectangleDemo::new(Kind::Section, "section-rounded-gap".into(), cx)
        });
        let view = handle.root(cx).unwrap();
        let mut visual = VisualTestContext::from_window(*handle, cx);
        visual.run_until_parked();
        assert!(visual.debug_bounds("section-rounded-gap-seam-0").is_some());
        view.update(cx, |view, cx| {
            let fixture = view.section_fixture.as_mut().unwrap();
            let second = fixture.project().arrangement_sections()[1].id();
            fixture
                .commit(Command::SetArrangementSectionRange {
                    section: second,
                    start: hex_project::BeatTime::from_raw(
                        5 * hex_project::BeatTime::ONE.raw() + 1,
                    ),
                    end: hex_project::BeatTime::from_quarters(9).unwrap(),
                })
                .unwrap();
            view.canonical = fixture.display();
            view.preview = view.canonical.clone();
            assert_eq!(view.canonical[0][1], view.canonical[1][0]);
            cx.notify();
        });
        visual.run_until_parked();
        assert!(visual.debug_bounds("section-rounded-gap-seam-0").is_none());
    }

    #[gpui::test]
    fn reorder_uses_the_editor_pointer_frame_at_insertion_ties(cx: &mut TestAppContext) {
        let handle = cx.add_window(|_, cx| {
            RectangleDemo::new(Kind::Section, "section-reorder-pointer".into(), cx)
        });
        let view = handle.root(cx).unwrap();
        // Preserve the float/frame insertion-tie regression in a true gap;
        // the old midpoint inside the second section now means replacement.
        view.update(cx, |view, cx| {
            let fixture = view.section_fixture.as_mut().unwrap();
            let id = fixture.project().arrangement_sections()[1].id();
            fixture
                .commit(Command::SetArrangementSectionRange {
                    section: id,
                    start: BeatTime::from_quarters(9).unwrap(),
                    end: BeatTime::from_quarters(10).unwrap(),
                })
                .unwrap();
            view.canonical = fixture.display();
            view.preview = view.canonical.clone();
            cx.notify();
        });
        let mut visual = VisualTestContext::from_window(*handle, cx);
        visual.update(|window, _| window.activate_window());
        visual.run_until_parked();
        let area = visual.debug_bounds("section-reorder-pointer-area").unwrap();
        let first = visual
            .debug_bounds("section-reorder-pointer-rectangle-0")
            .unwrap();
        let left = f32::from(area.left());
        let width = f32::from(area.size.width);
        let midpoint = left + width * 0.7;
        let (x, target, after) = view.read_with(cx, |view, _| {
            let fixture = view.section_fixture.as_ref().unwrap();
            let project = fixture.project();
            let source = project.arrangement_sections()[0].id();
            (0..256)
                .find_map(|offset| {
                    let x = f32::from_bits(midpoint.to_bits().checked_add(offset)?);
                    let exact = fixture.snapped_beat(x, left, width, true).ok()?;
                    let expected =
                        crate::section_reorder::insertion_target(project, source, exact, true)?;
                    let old = fixture
                        .plan(source, RectanglePart::Body, (x - left) / width, true, false)
                        .ok()??;
                    let Command::DuplicateArrangementSectionAt { target, after, .. } = old.command
                    else {
                        return None;
                    };
                    ((target, after) != expected).then_some((x, expected.0, expected.1))
                })
                .expect("fixture exposes the old float conversion mismatch")
        });
        let alt = Modifiers {
            alt: true,
            ..Modifiers::default()
        };
        let down = point(first.center().x, first.top() + px(5.0));
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(point(px(x), down.y), MouseButton::Left, alt);
        view.read_with(cx, |view, _| {
            assert!(matches!(view.section_command, Some(Command::DuplicateArrangementSectionAt { target: actual, after: side, .. }) if actual == target && side == after));
        });
    }

    #[gpui::test]
    fn option_drag_copies_real_section_and_tracks_live_modifiers(cx: &mut TestAppContext) {
        let handle =
            cx.add_window(|_, cx| RectangleDemo::new(Kind::Section, "section-copy".into(), cx));
        let view = handle.root(cx).unwrap();
        let mut visual = VisualTestContext::from_window(*handle, cx);
        visual.update(|window, _| window.activate_window());
        visual.run_until_parked();
        let area = visual.debug_bounds("section-copy-area").unwrap();
        let first = visual.debug_bounds("section-copy-rectangle-0").unwrap();
        let down = point(first.center().x, first.top() + px(5.0));
        let dest = point(area.left() + area.size.width * 0.9 - px(1.0), down.y);
        let alt = Modifiers {
            alt: true,
            ..Modifiers::default()
        };
        let original = view.read_with(cx, |view, _| {
            view.section_fixture.as_ref().unwrap().project().snapshot()
        });
        let original_ids = view.read_with(cx, |view, _| {
            view.section_fixture
                .as_ref()
                .unwrap()
                .project()
                .arrangement_sections()
                .iter()
                .map(|section| section.id())
                .collect::<Vec<_>>()
        });
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(dest, MouseButton::Left, alt);
        visual.simulate_keystrokes("escape");
        visual.run_until_parked();
        view.read_with(cx, |view, _| {
            assert!(view.capture.is_none());
            assert!(view.pending.is_none());
            assert_eq!(
                view.section_fixture.as_ref().unwrap().project().snapshot(),
                original
            );
        });
        for accept in [false, true] {
            visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_move(dest, MouseButton::Left, Modifiers::default());
            view.read_with(cx, |view, _| {
                assert!(matches!(
                    view.section_command,
                    Some(Command::ReorderSectionAfter { .. })
                ))
            });
            visual.simulate_modifiers_change(alt);
            view.read_with(cx, |view, _| {
                assert!(matches!(
                    view.section_command,
                    Some(Command::DuplicateArrangementSectionAt { .. })
                ));
                let selected: Vec<_> = view
                    .section_preview
                    .iter()
                    .filter(|section| view.section_selected(section))
                    .collect();
                assert_eq!(selected.len(), 1);
                assert!(
                    view.section_fixture
                        .as_ref()
                        .unwrap()
                        .project()
                        .arrangement_section(selected[0].id())
                        .is_none()
                );
            });
            visual.simulate_modifiers_change(Modifiers::default());
            view.read_with(cx, |view, _| {
                assert!(matches!(
                    view.section_command,
                    Some(Command::ReorderSectionAfter { .. })
                ));
            });
            visual.simulate_modifiers_change(alt);
            visual.simulate_mouse_up(dest, MouseButton::Left, alt);
            visual.run_until_parked();
            view.read_with(cx, |view, _| {
                let Some(Pending::Section {
                    sections, command, ..
                }) = view.pending.as_ref()
                else {
                    panic!("copy preview should remain pending");
                };
                assert!(matches!(
                    command.as_ref(),
                    Command::DuplicateArrangementSectionAt { .. }
                ));
                assert_eq!(
                    sections
                        .iter()
                        .filter(|section| view.section_selected(section))
                        .count(),
                    1
                );
            });
            view.update(cx, |view, cx| view.settle(accept, cx));
            visual.run_until_parked();
            view.read_with(cx, |view, _| {
                let project = view.section_fixture.as_ref().unwrap().project();
                if accept {
                    assert_eq!(project.arrangement_sections().len(), 3);
                    let selected = view.selection_primary.unwrap();
                    assert_eq!(view.selected, [selected]);
                    assert!(!original_ids.contains(&project.arrangement_sections()[selected].id()));
                } else {
                    assert_eq!(project.snapshot(), original);
                }
            });
        }
    }

    #[gpui::test]
    fn option_command_seam_drag_latches_marker_only_until_terminal(cx: &mut TestAppContext) {
        let handle = cx.add_window(|_, cx| {
            RectangleDemo::new(Kind::Section, "section-marker-edge".into(), cx)
        });
        let view = handle.root(cx).unwrap();
        let mut visual = VisualTestContext::from_window(*handle, cx);
        visual.update(|window, _| window.activate_window());
        visual.run_until_parked();
        let area = visual.debug_bounds("section-marker-edge-area").unwrap();
        let at = |x: f32| point(area.left() + area.size.width * x, area.top() + px(5.0));
        let command = Modifiers {
            platform: true,
            alt: true,
            ..Modifiers::default()
        };
        let original = view.read_with(cx, |view, _| {
            view.section_fixture.as_ref().unwrap().project().snapshot()
        });
        let right_start = view.read_with(cx, |view, _| {
            view.section_fixture
                .as_ref()
                .unwrap()
                .project()
                .arrangement_sections()[1]
                .start()
        });
        visual.simulate_mouse_down(at(0.5), MouseButton::Left, command);
        visual.simulate_mouse_move(at(0.45), MouseButton::Left, command);
        view.read_with(cx, |view, _| {
            assert!(matches!(
                view.section_command,
                Some(Command::ResizeArrangementSectionMarker { .. })
            ));
        });
        visual.simulate_keystrokes("escape");
        view.read_with(cx, |view, _| {
            assert_eq!(
                view.section_fixture.as_ref().unwrap().project().snapshot(),
                original
            );
        });
        for accept in [false, true] {
            visual.simulate_mouse_down(at(0.5), MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_move(at(0.45), MouseButton::Left, Modifiers::default());
            view.read_with(cx, |view, _| {
                assert!(matches!(
                    view.section_command,
                    Some(Command::ResizeArrangementSectionBoundary { .. })
                ))
            });
            visual.simulate_modifiers_change(command);
            visual.simulate_modifiers_change(Modifiers::default());
            view.read_with(cx, |view, _| {
                assert!(matches!(
                    view.section_command,
                    Some(Command::ResizeArrangementSectionMarker {
                        edge: hex_project::TrimEdge::Right,
                        ..
                    })
                ))
            });
            visual.simulate_mouse_up(at(0.45), MouseButton::Left, Modifiers::default());
            visual.run_until_parked();
            view.read_with(cx, |view, _| {
                let sections = view.section_fixture.as_ref().unwrap().project().arrangement_sections();
                let Some(Pending::Section { hover, .. }) = view.pending.as_ref() else { panic!("marker edit should be pending") };
                assert_eq!(hover.held, [Some(sections[0].id()), None]);
                assert!(hover.pointer.is_some());
                assert!(matches!(view.pending.as_ref(), Some(Pending::Section { command, .. }) if matches!(command.as_ref(), Command::ResizeArrangementSectionMarker { .. })));
                assert_eq!(view.section_fixture.as_ref().unwrap().project().snapshot(), original);
            });
            view.update(cx, |view, cx| view.settle(accept, cx));
            visual.run_until_parked();
            view.read_with(cx, |view, _| {
                let project = view.section_fixture.as_ref().unwrap().project();
                assert_eq!(project.arrangement_sections()[1].start(), right_start);
                if accept {
                    assert_ne!(project.snapshot(), original);
                    assert!(project.arrangement_sections()[0].end() < right_start);
                } else {
                    assert_eq!(project.snapshot(), original);
                }
            });
        }
    }

    #[gpui::test]
    fn section_seam_drag_uses_adaptive_grid_and_live_control_inversion(cx: &mut TestAppContext) {
        let handle =
            cx.add_window(|_, cx| RectangleDemo::new(Kind::Section, "section-grid".into(), cx));
        let view = handle.root(cx).unwrap();
        let mut visual = VisualTestContext::from_window(*handle, cx);
        visual.update(|window, _| window.activate_window());
        visual.run_until_parked();
        let area = visual.debug_bounds("section-grid-area").unwrap();
        let at = |x: f32| point(area.left() + area.size.width * x, area.top() + px(5.0));
        let control = Modifiers {
            control: true,
            ..Modifiers::default()
        };
        visual.simulate_mouse_down(at(0.5), MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(at(0.43), MouseButton::Left, Modifiers::default());
        let (snapped, raw) = view.read_with(cx, |view, _| {
            let fixture = view.section_fixture.as_ref().unwrap();
            let width = f32::from(view.bounds.unwrap().size.width);
            let pointer = f32::from(at(0.43).x);
            let left = f32::from(view.bounds.unwrap().left());
            (
                fixture.snapped_beat(pointer, left, width, false).unwrap(),
                fixture.snapped_beat(pointer, left, width, true).unwrap(),
            )
        });
        assert_ne!(snapped, raw);
        view.read_with(cx, |view, _| assert!(matches!(view.section_command, Some(Command::ResizeArrangementSectionBoundary { boundary, .. }) if boundary == snapped)));
        visual.simulate_modifiers_change(control);
        view.read_with(cx, |view, _| assert!(matches!(view.section_command, Some(Command::ResizeArrangementSectionBoundary { boundary, .. }) if boundary == raw)));
        visual.simulate_mouse_up(at(0.43), MouseButton::Left, control);
        view.read_with(cx, |view, _| assert!(matches!(view.pending.as_ref(), Some(Pending::Section { command, .. }) if matches!(command.as_ref(), Command::ResizeArrangementSectionBoundary { boundary, .. } if *boundary == raw))));
        view.update(cx, |view, cx| view.settle(false, cx));
    }

    #[gpui::test]
    fn option_command_header_drag_latches_marker_movement_until_terminal(cx: &mut TestAppContext) {
        let handle = cx.add_window(|_, cx| {
            RectangleDemo::new(Kind::Section, "section-marker-move".into(), cx)
        });
        let view = handle.root(cx).unwrap();
        let mut visual = VisualTestContext::from_window(*handle, cx);
        visual.update(|window, _| window.activate_window());
        visual.run_until_parked();
        let area = visual.debug_bounds("section-marker-move-area").unwrap();
        let at = |x: f32| point(area.left() + area.size.width * x, area.top() + px(5.0));
        let command = Modifiers {
            platform: true,
            alt: true,
            ..Modifiers::default()
        };
        let (before, original_start, original_end, other) = view.read_with(cx, |view, _| {
            let project = view.section_fixture.as_ref().unwrap().project();
            (
                project.snapshot(),
                project.arrangement_sections()[0].start(),
                project.arrangement_sections()[0].end(),
                project.arrangement_sections()[1].clone(),
            )
        });
        visual.simulate_mouse_down(at(0.3), MouseButton::Left, command);
        visual.simulate_mouse_move(at(0.25), MouseButton::Left, command);
        visual.simulate_keystrokes("escape");
        view.read_with(cx, |view, _| {
            assert_eq!(
                view.section_fixture.as_ref().unwrap().project().snapshot(),
                before
            )
        });
        for (at_press, accept) in [(true, false), (false, true)] {
            visual.simulate_mouse_down(
                at(0.3),
                MouseButton::Left,
                if at_press {
                    command
                } else {
                    Modifiers::default()
                },
            );
            visual.simulate_mouse_move(
                at(0.25),
                MouseButton::Left,
                if at_press {
                    command
                } else {
                    Modifiers::default()
                },
            );
            if !at_press {
                visual.simulate_modifiers_change(command);
            }
            visual.simulate_modifiers_change(Modifiers::default());
            view.read_with(cx, |view, _| {
                assert!(matches!(
                    view.section_command,
                    Some(Command::MoveArrangementSectionMarker { .. })
                ))
            });
            visual.simulate_mouse_up(at(0.25), MouseButton::Left, Modifiers::default());
            visual.run_until_parked();
            view.read_with(cx, |view, _| {
                assert!(matches!(view.pending.as_ref(), Some(Pending::Section { command, .. }) if matches!(command.as_ref(), Command::MoveArrangementSectionMarker { .. })));
                assert_eq!(view.section_fixture.as_ref().unwrap().project().snapshot(), before);
            });
            view.update(cx, |view, cx| view.settle(accept, cx));
            visual.run_until_parked();
            view.read_with(cx, |view, _| {
                let project = view.section_fixture.as_ref().unwrap().project();
                assert_eq!(project.arrangement_sections()[1], other);
                if accept {
                    let moved = &project.arrangement_sections()[0];
                    assert!(moved.start() < original_start);
                    assert_eq!(
                        moved.end().raw() - moved.start().raw(),
                        original_end.raw() - original_start.raw()
                    );
                } else {
                    assert_eq!(project.snapshot(), before);
                }
            });
        }
    }

    #[gpui::test]
    fn marker_move_freezes_press_grid_and_inverts_current_pointer_live(cx: &mut TestAppContext) {
        let handle = cx.add_window(|_, cx| {
            RectangleDemo::new(Kind::Section, "section-marker-grid".into(), cx)
        });
        let view = handle.root(cx).unwrap();
        let mut visual = VisualTestContext::from_window(*handle, cx);
        visual.update(|window, _| window.activate_window());
        visual.run_until_parked();
        let area = visual.debug_bounds("section-marker-grid-area").unwrap();
        let at = |x: f32| point(area.left() + area.size.width * x, area.top() + px(5.0));
        let both = Modifiers {
            platform: true,
            alt: true,
            control: true,
            ..Modifiers::default()
        };
        let command = Modifiers {
            platform: true,
            alt: true,
            ..Modifiers::default()
        };
        let control = Modifiers {
            control: true,
            ..Modifiers::default()
        };
        let (anchor, snapped, free, original_start) = view.read_with(cx, |view, _| {
            let fixture = view.section_fixture.as_ref().unwrap();
            let bounds = view.bounds.unwrap();
            let left = f32::from(bounds.left());
            let width = f32::from(bounds.size.width);
            (
                fixture
                    .snapped_beat(at(0.33).x.into(), left, width, true)
                    .unwrap(),
                fixture
                    .snapped_beat(at(0.263).x.into(), left, width, false)
                    .unwrap(),
                fixture
                    .snapped_beat(at(0.263).x.into(), left, width, true)
                    .unwrap(),
                fixture.project().arrangement_sections()[0].start(),
            )
        });
        assert_ne!(snapped, free);
        let expected_start = |position: BeatTime| {
            BeatTime::from_raw(
                u64::try_from(
                    i128::from(original_start.raw()) + i128::from(position.raw())
                        - i128::from(anchor.raw()),
                )
                .unwrap(),
            )
        };
        visual.simulate_mouse_down(at(0.33), MouseButton::Left, both);
        visual.simulate_mouse_move(at(0.263), MouseButton::Left, command);
        view.read_with(cx, |view, _| {
            assert_eq!(view.section_anchor_beat, Some(anchor));
            assert!(matches!(view.section_command, Some(Command::MoveArrangementSectionMarker { start, .. }) if start == expected_start(snapped)));
        });
        visual.simulate_modifiers_change(control);
        view.read_with(cx, |view, _| {
            assert_eq!(view.section_anchor_beat, Some(anchor));
            assert!(matches!(view.section_command, Some(Command::MoveArrangementSectionMarker { start, .. }) if start == expected_start(free)));
        });
        visual.simulate_mouse_up(at(0.263), MouseButton::Left, control);
        view.read_with(cx, |view, _| assert!(matches!(view.pending.as_ref(), Some(Pending::Section { command, .. }) if matches!(command.as_ref(), Command::MoveArrangementSectionMarker { start, .. } if *start == expected_start(free)))));
        view.update(cx, |view, cx| view.settle(true, cx));
        view.read_with(cx, |view, _| {
            assert_eq!(
                view.section_fixture
                    .as_ref()
                    .unwrap()
                    .project()
                    .arrangement_sections()[0]
                    .start(),
                expected_start(free)
            )
        });
    }

    #[gpui::test]
    fn outside_release_uses_last_valid_section_location_and_live_copy_modifier(
        cx: &mut TestAppContext,
    ) {
        let handle = cx.add_window(|_, cx| {
            RectangleDemo::new(Kind::Section, "section-outside-copy".into(), cx)
        });
        let view = handle.root(cx).unwrap();
        let mut visual = VisualTestContext::from_window(*handle, cx);
        visual.update(|window, _| window.activate_window());
        visual.run_until_parked();
        let area = visual.debug_bounds("section-outside-copy-area").unwrap();
        let at = |x: f32| point(area.left() + area.size.width * x, area.top() + px(5.0));
        let outside = point(area.right() + px(50.0), area.top() - px(10.0));
        let alt = Modifiers {
            alt: true,
            ..Modifiers::default()
        };
        let original = view.read_with(cx, |view, _| {
            view.section_fixture.as_ref().unwrap().project().snapshot()
        });
        visual.simulate_mouse_down(at(0.3), MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(
            point(at(0.9).x - px(1.0), at(0.9).y),
            MouseButton::Left,
            Modifiers::default(),
        );
        visual.simulate_mouse_move(outside, MouseButton::Left, alt);
        visual.simulate_keystrokes("escape");
        view.read_with(cx, |view, _| {
            assert!(view.capture.is_none());
            assert!(view.pending.is_none());
            assert_eq!(
                view.section_fixture.as_ref().unwrap().project().snapshot(),
                original
            );
        });
        for accept in [false, true] {
            visual.simulate_mouse_down(at(0.3), MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_move(
                point(at(0.9).x - px(1.0), at(0.9).y),
                MouseButton::Left,
                Modifiers::default(),
            );
            visual.simulate_mouse_move(outside, MouseButton::Left, Modifiers::default());
            visual.simulate_modifiers_change(alt);
            let expected = view.read_with(cx, |view, _| {
                let fixture = view.section_fixture.as_ref().unwrap();
                let id = fixture.project().arrangement_sections()[0].id();
                let bounds = view.bounds.unwrap();
                let position = (f32::from(point(at(0.9).x - px(1.0), at(0.9).y).x)
                    - f32::from(bounds.left()))
                    / f32::from(bounds.size.width);
                fixture
                    .plan(id, RectanglePart::Body, position, true, false)
                    .unwrap()
                    .unwrap()
                    .sections
            });
            view.read_with(cx, |view, _| {
                assert_eq!(view.section_preview, expected);
                assert!(matches!(
                    view.section_command,
                    Some(Command::DuplicateArrangementSectionAt { .. })
                ));
            });
            visual.simulate_mouse_up(outside, MouseButton::Left, alt);
            visual.run_until_parked();
            view.read_with(cx, |view, _| {
                assert!(matches!(view.pending.as_ref(), Some(Pending::Section { sections, .. }) if *sections == expected));
                assert_eq!(view.section_fixture.as_ref().unwrap().project().snapshot(), original);
            });
            view.update(cx, |view, cx| view.settle(accept, cx));
            visual.run_until_parked();
            view.read_with(cx, |view, _| {
                let project = view.section_fixture.as_ref().unwrap().project();
                if accept {
                    assert_eq!(project.arrangement_sections().len(), 3);
                } else {
                    assert_eq!(project.snapshot(), original);
                }
            });
        }
    }

    #[gpui::test]
    fn outside_modifier_change_latches_marker_move_at_last_valid_position(cx: &mut TestAppContext) {
        let handle = cx.add_window(|_, cx| {
            RectangleDemo::new(Kind::Section, "section-outside-marker".into(), cx)
        });
        let view = handle.root(cx).unwrap();
        let mut visual = VisualTestContext::from_window(*handle, cx);
        visual.update(|window, _| window.activate_window());
        visual.run_until_parked();
        let area = visual.debug_bounds("section-outside-marker-area").unwrap();
        let at = |x: f32| point(area.left() + area.size.width * x, area.top() + px(5.0));
        let outside = point(area.right() + px(50.0), area.top() - px(10.0));
        let command = Modifiers {
            platform: true,
            alt: true,
            ..Modifiers::default()
        };
        visual.simulate_mouse_down(at(0.3), MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(at(0.25), MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(outside, MouseButton::Left, Modifiers::default());
        visual.simulate_modifiers_change(command);
        let preview = view.read_with(cx, |view, _| {
            assert!(matches!(
                view.section_command,
                Some(Command::MoveArrangementSectionMarker { .. })
            ));
            view.section_preview.clone()
        });
        visual.simulate_modifiers_change(Modifiers::default());
        visual.simulate_mouse_up(outside, MouseButton::Left, Modifiers::default());
        view.read_with(cx, |view, _| assert!(matches!(view.pending.as_ref(), Some(Pending::Section { sections, command, .. }) if *sections == preview && matches!(command.as_ref(), Command::MoveArrangementSectionMarker { .. }))));
        view.update(cx, |view, cx| view.settle(true, cx));
        view.read_with(cx, |view, _| {
            assert_eq!(
                view.section_fixture
                    .as_ref()
                    .unwrap()
                    .project()
                    .arrangement_sections(),
                preview
            )
        });
    }

    #[gpui::test]
    fn double_click_paint_retains_real_preview_until_terminal(cx: &mut TestAppContext) {
        let handle =
            cx.add_window(|_, cx| RectangleDemo::new(Kind::Section, "section-paint".into(), cx));
        let view = handle.root(cx).unwrap();
        let mut visual = VisualTestContext::from_window(*handle, cx);
        visual.update(|window, _| window.activate_window());
        visual.run_until_parked();
        let area = visual.debug_bounds("section-paint-area").unwrap();
        let at = |x: f32| {
            point(
                area.left() + area.size.width * x,
                area.top() + area.size.height * 0.9,
            )
        };
        let outside = point(area.right() + px(20.0), area.top() - px(10.0));
        let double_down = |visual: &mut VisualTestContext| {
            visual.simulate_event(gpui::MouseDownEvent {
                position: at(0.92),
                button: MouseButton::Left,
                modifiers: Modifiers::default(),
                click_count: 2,
                first_mouse: false,
            })
        };
        let before = view.read_with(cx, |view, _| {
            view.section_fixture.as_ref().unwrap().project().snapshot()
        });
        visual.simulate_mouse_down(at(0.92), MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(at(0.92), MouseButton::Left, Modifiers::default());
        view.read_with(cx, |view, _| {
            assert!(view.capture.is_none() && view.pending.is_none())
        });
        double_down(&mut visual);
        visual.simulate_mouse_up(at(0.92), MouseButton::Left, Modifiers::default());
        view.read_with(cx, |view, _| assert!(matches!(view.pending.as_ref(), Some(Pending::Section { command, .. }) if matches!(command.as_ref(), Command::AddArrangementSection { .. }))));
        view.update(cx, |view, cx| view.settle(false, cx));
        double_down(&mut visual);
        visual.simulate_mouse_move(at(0.5), MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(at(0.5), MouseButton::Left, Modifiers::default());
        view.read_with(cx, |view, _| {
            assert!(view.pending.is_none());
            assert_eq!(
                view.section_fixture.as_ref().unwrap().project().snapshot(),
                before
            );
        });
        double_down(&mut visual);
        visual.simulate_mouse_move(at(0.96), MouseButton::Left, Modifiers::default());
        visual.simulate_keystrokes("escape");
        view.read_with(cx, |view, _| {
            assert_eq!(
                view.section_fixture.as_ref().unwrap().project().snapshot(),
                before
            )
        });
        for accept in [false, true] {
            double_down(&mut visual);
            let appearance = view.read_with(cx, |view, _| view.section_paint.clone().unwrap());
            visual.simulate_mouse_move(at(0.96), MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_move(outside, MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_up(outside, MouseButton::Left, Modifiers::default());
            let released = view.update(cx, |view, _| {
                let Some(Pending::Section { sections, command, receipt, hover, .. }) = view.pending.as_mut() else { panic!("paint should be pending"); };
                assert_eq!(hover.held, [None, None]);
                assert!(hover.pointer.is_some());
                assert!(matches!(command.as_ref(), Command::AddArrangementSection { name, color, .. } if *name == appearance.0 && *color == appearance.1));
                *receipt = crate::pending_edit::Receipt::new(1);
                assert!(receipt.submitted(2));
                sections.clone()
            });
            view.update(cx, |view, cx| {
                view.receive_section(1, false, true, accept, None, cx)
            });
            visual.run_until_parked();
            view.read_with(cx, |view, _| {
                assert!(matches!(view.pending.as_ref(), Some(Pending::Section { sections, .. }) if *sections == released));
                assert_eq!(view.section_fixture.as_ref().unwrap().project().snapshot(), before);
            });
            view.update(cx, |view, cx| {
                view.receive_section(1, false, true, accept, None, cx)
            });
            visual.run_until_parked();
            view.read_with(cx, |view, _| {
                let project = view.section_fixture.as_ref().unwrap().project();
                if accept {
                    assert_eq!(project.arrangement_sections(), released);
                } else {
                    assert_eq!(project.snapshot(), before);
                }
            });
        }
    }

    #[gpui::test]
    fn paint_freezes_press_grid_and_uses_live_pointer_grid(cx: &mut TestAppContext) {
        let handle = cx
            .add_window(|_, cx| RectangleDemo::new(Kind::Section, "section-paint-grid".into(), cx));
        let view = handle.root(cx).unwrap();
        let mut visual = VisualTestContext::from_window(*handle, cx);
        visual.update(|window, _| window.activate_window());
        visual.run_until_parked();
        let area = visual.debug_bounds("section-paint-grid-area").unwrap();
        let at = |x: f32| point(area.left() + area.size.width * x, area.top() + px(5.0));
        let control = Modifiers {
            control: true,
            ..Modifiers::default()
        };
        let (anchor, point_end, destination) = view.read_with(cx, |view, _| {
            let fixture = view.section_fixture.as_ref().unwrap();
            let bounds = view.bounds.unwrap();
            let left = f32::from(bounds.left());
            let width = f32::from(bounds.size.width);
            let anchor = fixture
                .snapped_beat(at(0.923).x.into(), left, width, true)
                .unwrap();
            (
                anchor,
                fixture.paint_point_end(anchor, width, true).unwrap(),
                fixture
                    .snapped_beat(at(0.965).x.into(), left, width, false)
                    .unwrap(),
            )
        });
        visual.simulate_event(gpui::MouseDownEvent {
            position: at(0.923),
            button: MouseButton::Left,
            modifiers: control,
            click_count: 2,
            first_mouse: false,
        });
        view.read_with(cx, |view, _| {
            assert_eq!(view.section_anchor_beat, Some(anchor));
            assert!(matches!(view.section_command, Some(Command::AddArrangementSection { start, end, .. }) if start == anchor && end == point_end));
        });
        visual.simulate_mouse_move(at(0.965), MouseButton::Left, Modifiers::default());
        view.read_with(cx, |view, _| {
            assert_eq!(view.section_anchor_beat, Some(anchor));
            assert!(matches!(view.section_command, Some(Command::AddArrangementSection { start, end, .. }) if start == anchor.min(destination) && end == anchor.max(destination)));
        });
        visual.simulate_mouse_up(at(0.965), MouseButton::Left, Modifiers::default());
        view.read_with(cx, |view, _| assert!(matches!(view.pending.as_ref(), Some(Pending::Section { command, .. }) if matches!(command.as_ref(), Command::AddArrangementSection { start, end, .. } if *start == anchor.min(destination) && *end == anchor.max(destination)))));
        view.update(cx, |view, cx| view.settle(true, cx));
        view.read_with(cx, |view, _| {
            assert!(
                view.section_fixture
                    .as_ref()
                    .unwrap()
                    .project()
                    .arrangement_sections()
                    .iter()
                    .any(|section| section.start() == anchor.min(destination)
                        && section.end() == anchor.max(destination))
            )
        });
    }

    #[gpui::test]
    fn shift_selection_uses_exact_section_starts_not_display_floats(cx: &mut TestAppContext) {
        let handle = cx.add_window(|_, cx| {
            RectangleDemo::new(Kind::Section, "section-exact-selection".into(), cx)
        });
        let view = handle.root(cx).unwrap();
        view.update(cx, |view, cx| {
            let fixture = view.section_fixture.as_mut().unwrap();
            let ids: Vec<_> = fixture
                .project()
                .arrangement_sections()
                .iter()
                .map(|s| s.id())
                .collect();
            for id in ids {
                fixture
                    .commit(Command::RemoveArrangementSection(id))
                    .unwrap();
            }
            let first = BeatTime::ONE;
            let second = BeatTime::from_raw(first.raw() + 1);
            for (name, start, end) in [
                ("a", first, second),
                ("b", second, BeatTime::from_quarters(2).unwrap()),
                (
                    "c",
                    BeatTime::from_quarters(5).unwrap(),
                    BeatTime::from_quarters(9).unwrap(),
                ),
            ] {
                fixture
                    .commit(Command::AddArrangementSection {
                        name: name.into(),
                        color: hex_project::ElementColor::Cyan,
                        start,
                        end,
                    })
                    .unwrap();
            }
            view.canonical = fixture.display();
            assert_eq!(view.canonical[0][0], view.canonical[1][0]);
            view.preview = view.canonical.clone();
            view.section_preview = fixture.project().arrangement_sections().to_vec();
            view.selected = vec![1];
            view.selection_primary = Some(1);
            view.selection_anchor = Some(1);
            cx.notify();
        });
        let mut visual = VisualTestContext::from_window(*handle, cx);
        visual.update(|window, _| window.activate_window());
        visual.run_until_parked();
        let last = visual
            .debug_bounds("section-exact-selection-rectangle-2")
            .unwrap();
        let shift = Modifiers {
            shift: true,
            ..Modifiers::default()
        };
        visual.simulate_mouse_down(
            point(last.center().x, last.top() + px(5.0)),
            MouseButton::Left,
            shift,
        );
        view.read_with(cx, |view, _| assert_eq!(view.selected, [1, 2]));
    }

    #[gpui::test]
    fn section_fixture_renders_and_selects_a_third_real_section(cx: &mut TestAppContext) {
        let handle =
            cx.add_window(|_, cx| RectangleDemo::new(Kind::Section, "section-third".into(), cx));
        let view = handle.root(cx).unwrap();
        view.update(cx, |view, cx| {
            let fixture = view.section_fixture.as_mut().unwrap();
            let id = fixture.project().arrangement_sections()[0].id();
            fixture
                .commit(Command::DuplicateArrangementSectionAt {
                    section: id,
                    target: id,
                    after: true,
                })
                .unwrap();
            view.canonical = fixture.display();
            view.preview = view.canonical.clone();
            view.section_preview = fixture.project().arrangement_sections().to_vec();
            cx.notify();
        });
        let mut visual = VisualTestContext::from_window(*handle, cx);
        visual.update(|window, _| window.activate_window());
        visual.run_until_parked();
        let bounds = visual.debug_bounds("section-third-rectangle-2").unwrap();
        visual.simulate_mouse_down(
            point(bounds.center().x, bounds.top() + px(5.0)),
            MouseButton::Left,
            Modifiers::default(),
        );
        view.read_with(cx, |view, _| assert_eq!(view.selected, [2]));
    }

    #[gpui::test]
    fn rectangle_fixtures_hold_pending_across_repaints_cancel_and_refuse(cx: &mut TestAppContext) {
        for section in [false, true] {
            let handle = cx.add_window(|_, cx| {
                RectangleDemo::new(
                    if section { Kind::Section } else { Kind::Clip },
                    "rectangle-fixture".into(),
                    cx,
                )
            });
            let view = handle.root(cx).unwrap();
            let mut visual = VisualTestContext::from_window(*handle, cx);
            visual.update(|window, _| window.activate_window());
            visual.run_until_parked();
            let bounds = visual
                .debug_bounds("rectangle-fixture-rectangle-0")
                .unwrap();
            let down = point(
                bounds.left() + bounds.size.width * 0.5,
                bounds.top() + px(5.0),
            );
            let target = if section {
                let area = visual.debug_bounds("rectangle-fixture-area").unwrap();
                point(area.left() + area.size.width * 0.95, down.y)
            } else {
                point(down.x + px(20.0), down.y)
            };
            let before = view.read_with(cx, |view, _| view.canonical.clone());
            let before_project = view.read_with(cx, |view, _| {
                view.section_fixture
                    .as_ref()
                    .map(|fixture| fixture.project().snapshot())
            });
            visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
            visual.run_until_parked();
            view.read_with(cx, |view, _| {
                assert!(view.capture.is_some());
                if section {
                    assert!(view.section_command.is_some());
                    assert_ne!(
                        view.section_preview[0].id(),
                        view.section_fixture
                            .as_ref()
                            .unwrap()
                            .project()
                            .arrangement_sections()[0]
                            .id()
                    );
                } else {
                    assert_ne!(view.preview, before);
                }
                assert_eq!(view.canonical, before);
            });
            visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
            visual.run_until_parked();
            let released = visual
                .debug_bounds("rectangle-fixture-rectangle-0")
                .unwrap();
            view.read_with(cx, |view, _| {
                assert!(view.capture.is_none());
                assert!(view.pending.is_some());
                assert_eq!(view.canonical, before);
            });
            for _ in 0..3 {
                view.update(cx, |_, cx| cx.notify());
                visual.run_until_parked();
                assert_eq!(
                    visual
                        .debug_bounds("rectangle-fixture-rectangle-0")
                        .unwrap(),
                    released
                );
            }
            if section {
                let pending =
                    view.read_with(cx, |view, _| view.pending.as_ref().map(Pending::display));
                let body = point(
                    released.center().x,
                    released.top() + released.size.height * 0.75,
                );
                visual.simulate_mouse_down(body, MouseButton::Left, Modifiers::default());
                view.read_with(cx, |view, _| {
                    assert_eq!(view.pending.as_ref().map(Pending::display), pending);
                    assert!(view.capture.is_none());
                    assert!(view.range.is_none());
                });
            }
            visual.simulate_keystrokes("escape");
            view.update(cx, |view, cx| view.set_enabled(false, cx));
            visual.run_until_parked();
            view.read_with(cx, |view, _| {
                assert!(
                    view.pending.is_some(),
                    "cancel/disable cannot retire admitted paint"
                )
            });
            view.update(cx, |view, cx| {
                view.settle(false, cx);
                view.set_enabled(true, cx);
            });
            visual.run_until_parked();
            assert_eq!(
                visual
                    .debug_bounds("rectangle-fixture-rectangle-0")
                    .unwrap(),
                bounds
            );
            visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
            visual.simulate_keystrokes("escape");
            visual.run_until_parked();
            view.read_with(cx, |view, _| {
                assert!(view.capture.is_none());
                assert_eq!(view.canonical, before);
                assert_eq!(view.preview, before);
            });
            // Lost-button input cancels; a foreign release cannot finish capture.
            visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_up(target, MouseButton::Right, Modifiers::default());
            view.read_with(cx, |view, _| assert!(view.capture.is_some()));
            visual.simulate_mouse_move(target, None, Modifiers::default());
            view.read_with(cx, |view, _| assert!(view.capture.is_none()));
            // Acceptance installs exactly the released display, after which a
            // fresh gesture starts from the newly installed geometry.
            visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
            let pending = view.read_with(cx, |view, _| {
                view.pending.as_ref().map(Pending::display).unwrap()
            });
            view.update(cx, |view, cx| view.settle(true, cx));
            visual.run_until_parked();
            view.read_with(cx, |view, _| {
                assert_eq!(view.canonical, pending);
                if section {
                    assert_ne!(
                        view.section_fixture
                            .as_ref()
                            .map(|fixture| fixture.project().snapshot()),
                        before_project
                    );
                }
                assert!(view.pending.is_none());
            });
        }
    }
    #[gpui::test]
    fn rectangle_fixtures_render_same_narrow_edges_seams_and_focus_in_both_themes(
        cx: &mut TestAppContext,
    ) {
        struct Restore(hex_preferences::ThemePreferences);
        impl Drop for Restore {
            fn drop(&mut self) {
                theme::apply_preferences(self.0);
            }
        }
        let _restore = Restore(theme::preferences());
        for light in [false, true] {
            theme::apply_preferences(if light {
                hex_preferences::ThemePreferences {
                    background: hex_preferences::RgbColor::new(225, 229, 238),
                    text: hex_preferences::RgbColor::new(24, 27, 34),
                    primary: hex_preferences::RgbColor::new(145, 25, 45),
                    secondary: hex_preferences::RgbColor::new(16, 70, 145),
                }
            } else {
                hex_preferences::ThemePreferences::default()
            });
            for section in [false, true] {
                let handle = cx.add_window(|_, cx| {
                    RectangleDemo::new(
                        if section { Kind::Section } else { Kind::Clip },
                        "rectangle-narrow".into(),
                        cx,
                    )
                });
                let view = handle.root(cx).unwrap();
                let mut visual = VisualTestContext::from_window(*handle, cx);
                visual.update(|window, _| window.activate_window());
                visual.simulate_resize(gpui::size(px(240.0), px(240.0)));
                visual.simulate_mouse_move(point(px(1.0), px(1.0)), None, Modifiers::default());
                visual.run_until_parked();
                let a = visual.debug_bounds("rectangle-narrow-rectangle-0").unwrap();
                let b = visual.debug_bounds("rectangle-narrow-rectangle-1").unwrap();
                if section {
                    let header = visual
                        .debug_bounds("rectangle-narrow-section-header-0")
                        .unwrap();
                    assert_eq!(header.size.height, px(14.0));
                    assert!(header.bottom() < a.bottom());
                    let (fill, _, _) =
                        theme::section_colors(hex_project::ElementColor::Cyan, false, false);
                    assert_eq!(
                        painted_surface(&mut visual, "rectangle-narrow-section-header-0"),
                        Some(gpui::rgb(fill).into())
                    );
                    assert_eq!(
                        painted_surface(&mut visual, "rectangle-narrow-rectangle-0"),
                        Some(gpui::rgb(fill).into())
                    );
                }
                let at = point(a.left() + px(2.0), a.top() + px(20.0));
                visual.simulate_mouse_move(at, None, Modifiers::default());
                visual.run_until_parked();
                assert_eq!(
                    visual.rendered_cursor_style(),
                    Some(RectanglePart::Leading.cursor()),
                    "section={section}, light={light}, a={a:?}, b={b:?}, at={at:?}"
                );
                if section {
                    let at = point(a.right(), a.top() + px(20.0));
                    visual.simulate_mouse_move(at, None, Modifiers::default());
                    visual.run_until_parked();
                    assert_eq!(
                        visual.rendered_cursor_style(),
                        Some(RectanglePart::Seam.cursor())
                    );
                    visual.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
                    visual.simulate_mouse_move(
                        point(at.x + px(10.0), at.y),
                        MouseButton::Left,
                        Modifiers::default(),
                    );
                    view.read_with(cx, |view, _| {
                        assert_eq!(view.capture.unwrap().part, RectanglePart::Seam);
                        assert_eq!(view.preview[0][1], view.preview[1][0]);
                    });
                    view.update(cx, |view, cx| view.set_enabled(false, cx));
                    view.read_with(cx, |view, _| {
                        assert!(view.capture.is_none());
                        assert_eq!(view.preview, view.canonical);
                    });
                } else {
                    let area = visual.debug_bounds("rectangle-narrow-area").unwrap();
                    assert!((b.left() - (area.left() + area.size.width * 0.7)).abs() <= px(0.25));
                    assert!((b.right() - (area.left() + area.size.width * 0.71)).abs() <= px(0.25));
                    assert!(
                        b.size.width < px(3.0),
                        "tiny clip is not widened for handles"
                    );
                    let at = point(b.left() + b.size.width * 0.5, b.top() + px(5.0));
                    visual.simulate_mouse_move(at, None, Modifiers::default());
                    visual.run_until_parked();
                    assert_eq!(
                        visual.rendered_cursor_style(),
                        Some(RectanglePart::Body.cursor())
                    );
                    visual.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
                    view.read_with(cx, |view, _| {
                        assert_eq!(view.capture.unwrap().part, RectanglePart::Body)
                    });
                    visual.run_until_parked();
                    assert!(
                        visual.update(|window, cx| view.read(cx).focus.is_focused(window)),
                        "capture focus before blur light={light}"
                    );
                    visual.update(|window, _| window.blur());
                    visual.run_until_parked();
                    view.read_with(cx, |view, _| {
                        assert!(
                            view.capture.is_none(),
                            "blur retires held input light={light}"
                        )
                    });
                }
                view.update(cx, |view, cx| view.set_enabled(false, cx));
                visual.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
                view.read_with(cx, |view, _| assert!(view.capture.is_none()));
            }
        }
    }
    #[gpui::test]
    fn beyond_end_drag_retains_gap_and_selection_through_pending_paints(cx: &mut TestAppContext) {
        for copy in [false, true] {
            let handle = cx
                .add_window(|_, cx| RectangleDemo::new(Kind::Section, "section-beyond".into(), cx));
            let view = handle.root(cx).unwrap();
            let mut visual = VisualTestContext::from_window(*handle, cx);
            visual.update(|window, _| window.activate_window());
            visual.run_until_parked();
            let area = visual.debug_bounds("section-beyond-area").unwrap();
            let last = visual.debug_bounds("section-beyond-rectangle-1").unwrap();
            let from = point(last.center().x, last.top() + px(5.0));
            let to = point(area.left() + area.size.width * 0.97, from.y);
            let modifiers = Modifiers {
                alt: copy,
                ..Modifiers::default()
            };
            let before = view.read_with(cx, |v, _| {
                v.section_fixture.as_ref().unwrap().project().snapshot()
            });
            visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_move(to, MouseButton::Left, modifiers);
            visual.run_until_parked();
            let appearance = view.read_with(cx, |v, _| {
                assert!(matches!(
                    v.section_command,
                    Some(
                        Command::MoveArrangementSectionBeyondEnd { .. }
                            | Command::AppendArrangementSectionCopy { .. }
                    )
                ));
                v.section_gap_appearance.clone().unwrap()
            });
            visual.simulate_mouse_up(to, MouseButton::Left, modifiers);
            visual.run_until_parked();
            let index = view.read_with(cx, |v, _| {
                let Some(Pending::Section { sections, .. }) = v.pending.as_ref() else {
                    panic!("pending section move/copy")
                };
                assert_eq!(
                    v.section_fixture.as_ref().unwrap().project().snapshot(),
                    before
                );
                let selected: Vec<_> = sections
                    .iter()
                    .enumerate()
                    .filter(|(_, m)| v.section_selected(m))
                    .collect();
                assert_eq!(selected.len(), 1);
                let gap = sections.iter().find(|m| m.name() == appearance.0).unwrap();
                assert_eq!(gap.color(), appearance.1);
                assert!(!v.section_selected(gap));
                assert!(gap.end() > BeatTime::from_quarters(9).unwrap());
                selected[0].0
            });
            let selector = Box::leak(format!("section-beyond-rectangle-{index}").into_boxed_str());
            let frozen = visual.debug_bounds(selector).unwrap();
            for _ in 0..3 {
                visual.update(|window, _| window.refresh());
                visual.run_until_parked();
                assert_eq!(visual.debug_bounds(selector).unwrap(), frozen);
            }
            view.update(cx, |v, cx| v.settle(true, cx));
            visual.run_until_parked();
            assert_eq!(visual.debug_bounds(selector).unwrap(), frozen);
            view.read_with(cx, |v, _| {
                assert!(v.pending.is_none());
                let project = v.section_fixture.as_ref().unwrap().project();
                assert_eq!(
                    project.arrangement_sections().len(),
                    if copy { 4 } else { 3 }
                );
                let selected = v.selection_primary.unwrap();
                assert_eq!(selected, index);
                assert_eq!(v.selected, [selected]);
                assert_ne!(
                    project.arrangement_sections()[selected].name(),
                    appearance.0
                );
            });
        }
    }
    #[gpui::test]
    fn section_body_replacement_retains_one_highlight_and_copied_selection(
        cx: &mut TestAppContext,
    ) {
        for copy in [false, true] {
            let handle = cx.add_window(|_, cx| {
                RectangleDemo::new(Kind::Section, "section-replace".into(), cx)
            });
            let view = handle.root(cx).unwrap();
            let mut visual = VisualTestContext::from_window(*handle, cx);
            visual.update(|w, _| w.activate_window());
            visual.run_until_parked();
            let first = visual.debug_bounds("section-replace-rectangle-0").unwrap();
            let second = visual.debug_bounds("section-replace-rectangle-1").unwrap();
            let from = point(first.center().x, first.top() + px(5.0));
            let to = point(second.center().x, from.y);
            let modifiers = Modifiers {
                alt: copy,
                ..Modifiers::default()
            };
            visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_move(to, MouseButton::Left, modifiers);
            visual.run_until_parked();
            assert!(
                visual
                    .debug_bounds("sandbox-section-drop-replace")
                    .is_some()
            );
            assert!(
                visual
                    .debug_bounds("sandbox-section-drop-insertion")
                    .is_none()
            );
            visual.simulate_mouse_up(to, MouseButton::Left, modifiers);
            visual.run_until_parked();
            let frozen = visual.debug_bounds("sandbox-section-drop-replace").unwrap();
            for _ in 0..3 {
                visual.update(|w, _| w.refresh());
                visual.run_until_parked();
                assert_eq!(
                    visual.debug_bounds("sandbox-section-drop-replace").unwrap(),
                    frozen
                );
            }
            view.update(cx, |v, cx| v.settle(true, cx));
            visual.run_until_parked();
            assert!(
                visual
                    .debug_bounds("sandbox-section-drop-replace")
                    .is_none()
            );
            view.read_with(cx, |v, _| {
                let p = v.section_fixture.as_ref().unwrap().project();
                assert_eq!(p.arrangement_sections().len(), if copy { 2 } else { 1 });
                let selected = v.selection_primary.unwrap();
                assert_eq!(v.selected, [selected]);
                assert_eq!(p.arrangement_sections()[selected].name(), "section 1");
            });
        }
    }
}
