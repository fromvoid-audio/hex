//! Time-range keyboard edits for one Arrangement device automation lane.
//! Points are copied at beat offsets from the selection start. Replacing or
//! inserting time always submits one validated lane edit.

use super::*;

#[derive(Clone, Copy)]
pub(in crate::editor) enum RangeEdit {
    Copy,
    Cut,
    Paste,
    Insert,
    Duplicate,
}

pub(super) struct Clipboard {
    source: Target,
    duration: u64,
    points: Vec<AutomationPoint>,
}

impl RangeSelection {
    pub(super) fn contains(self, beat: BeatTime) -> bool {
        self.start <= beat && beat < self.end
    }
}

impl Clipboard {
    fn capture(range: RangeSelection, points: &[AutomationPoint]) -> Result<Self, &'static str> {
        let duration = range.end.raw() - range.start.raw();
        let points: Vec<_> = points
            .iter()
            .filter(|point| range.contains(point.beat))
            .map(|point| AutomationPoint {
                beat: BeatTime::from_raw(point.beat.raw() - range.start.raw()),
                ..*point
            })
            .collect();
        if points.is_empty() {
            return Err("no automation points in the selected time range");
        }
        Ok(Self {
            source: range.target,
            duration,
            points,
        })
    }

    fn place(
        &self,
        at: BeatTime,
        insert: bool,
        original: &[AutomationPoint],
    ) -> Result<(RangeSelection, Vec<AutomationPoint>), &'static str> {
        let offset = |beat: BeatTime, distance| {
            beat.raw()
                .checked_add(distance)
                .map(BeatTime::from_raw)
                .ok_or("automation range exceeds the timeline")
        };
        let range = RangeSelection {
            target: self.source,
            start: at,
            end: offset(at, self.duration)?,
        };
        let mut points = Vec::new();
        for point in original {
            if insert && point.beat >= at {
                points.push(AutomationPoint {
                    beat: offset(point.beat, self.duration)?,
                    ..*point
                });
            } else if !range.contains(point.beat) {
                points.push(*point);
            }
        }
        for point in &self.points {
            points.push(AutomationPoint {
                beat: offset(at, point.beat.raw())?,
                ..*point
            });
        }
        if points.len() > hex_project::MAX_DEVICE_AUTOMATION_POINTS_PER_LANE {
            return Err("automation lane point limit reached");
        }
        points.sort_unstable_by_key(|point| point.beat);
        Ok((range, points))
    }
}

impl EditorView {
    pub(in crate::editor) fn clear_automation_range(&mut self) {
        let lanes = &mut self.automation_view.devices;
        if lanes.range.take().is_some() {
            lanes.selected = None;
        }
    }

    /// Once a lane owns a range, even a refused command stays in the lane.
    pub(in crate::editor) fn edit_automation_range(
        &mut self,
        edit: RangeEdit,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.mode != super::super::EditorMode::Arrangement
            || !self.automation_view.enabled
            || !self.focus.is_focused(window)
        {
            return false;
        }
        let Some(range) = self.automation_view.devices.range else {
            return true;
        };
        if let Err(reason) = self.apply_automation_range_edit(edit, range, cx) {
            self.error = Some(reason.into());
        }
        cx.notify();
        true
    }

    fn apply_automation_range_edit(
        &mut self,
        edit: RangeEdit,
        range: RangeSelection,
        cx: &mut Context<Self>,
    ) -> Result<(), &'static str> {
        let lanes = &self.automation_view.devices;
        if self.edit_pending()
            || lanes.pointer_active()
            || lanes.pending.is_some()
            || lanes.read_pending.is_some()
        {
            return Err("wait for the current automation edit");
        }
        let state = self
            .state
            .as_ref()
            .ok_or("automation project unavailable")?;
        let device = range
            .target
            .current(&state.project, self.project_generation)
            .ok_or("automation lane has changed")?;
        if lanes.entry(range.target).is_none() {
            return Err("automation lane is no longer visible");
        }
        let lane = device.automation_target(range.target.kind);
        if lane.is_some_and(|lane| !lane.touch.is_empty()) {
            return Err("clear recorded automation before editing this lane");
        }
        let points = lane.map_or(&[][..], |lane| lane.points.as_slice());
        match edit {
            RangeEdit::Copy | RangeEdit::Cut => {
                let copy = Clipboard::capture(range, points)?;
                if matches!(edit, RangeEdit::Cut) {
                    let remaining = points
                        .iter()
                        .copied()
                        .filter(|point| !range.contains(point.beat))
                        .collect();
                    self.commit_device_automation(range.target, remaining, cx);
                    if let Some(pending) = &mut self.automation_view.devices.pending {
                        pending.cut_clipboard = Some(copy);
                    }
                } else {
                    self.automation_view.devices.clipboard = Some(copy);
                }
            }
            RangeEdit::Duplicate => {
                let (destination, candidate) =
                    Clipboard::capture(range, points)?.place(range.end, false, points)?;
                self.commit_automation_range(destination, candidate, cx);
            }
            RangeEdit::Paste | RangeEdit::Insert => {
                let copy = lanes
                    .clipboard
                    .as_ref()
                    .ok_or("automation clipboard is empty")?;
                if copy.source != range.target {
                    return Err("automation clipboard belongs to a different lane");
                }
                let (destination, candidate) =
                    copy.place(range.start, matches!(edit, RangeEdit::Insert), points)?;
                self.commit_automation_range(destination, candidate, cx);
            }
        }
        Ok(())
    }

    fn commit_automation_range(
        &mut self,
        range: RangeSelection,
        points: Vec<AutomationPoint>,
        cx: &mut Context<Self>,
    ) {
        self.commit_device_automation(range.target, points, cx);
        if let Some(pending) = &mut self.automation_view.devices.pending {
            pending.select_range = Some(range);
        }
    }
}
