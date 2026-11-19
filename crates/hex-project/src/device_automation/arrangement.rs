//! Reassemble canonical device automation from consecutive half-open source
//! slices laid out in destination order. Section copies may repeat a source
//! slice. A two-point seam gives each adjacent curve its original limit without
//! inventing a one-tick ramp. Touch overlays retain their separately held values.
use super::{
    AutomationInterpolation, AutomationPoint, DeviceAutomation, DeviceAutomationTouchSpan,
    MAX_DEVICE_AUTOMATION_POINTS_PER_LANE, MAX_DEVICE_AUTOMATION_TOUCH_SPANS, base_sample, touch,
};
use crate::{BeatTime, Device, Invalid, Project};

struct Placement {
    start: BeatTime,
    end: Option<BeatTime>,
    destination: BeatTime,
}

impl Placement {
    fn map(&self, beat: BeatTime) -> Result<BeatTime, Invalid> {
        beat.raw()
            .checked_sub(self.start.raw())
            .and_then(|delta| self.destination.raw().checked_add(delta))
            .map(BeatTime::from_raw)
            .ok_or(Invalid("device automation section placement overflow"))
    }
}

/// Insert the source envelope interval together with the song-wide section
/// copy. Shift the original timeline after the insertion by the copy's length.
pub(crate) fn copy_section(
    project: &mut Project,
    start: BeatTime,
    end: BeatTime,
    insertion: BeatTime,
) -> Result<(), Invalid> {
    if !project.has_device_automation() {
        return Ok(());
    }
    let slices = [
        (BeatTime::ZERO, Some(insertion)),
        (start, Some(end)),
        (insertion, None),
    ];
    let mut next = BeatTime::ZERO;
    let mut placements = Vec::with_capacity(slices.len());
    for (start, end) in slices {
        if end == Some(start) {
            continue;
        }
        let placement = Placement {
            start,
            end,
            destination: next,
        };
        if let Some(end) = end {
            next = placement.map(end)?;
        }
        placements.push(placement);
    }

    let edit = |device: &mut Device| -> Result<(), Invalid> {
        for lane in &mut device.automation {
            assemble_lane(lane, &placements)?;
        }
        Ok(())
    };
    for device in project
        .tracks
        .iter_mut()
        .flat_map(|track| &mut track.devices)
        .chain(
            project
                .returns
                .iter_mut()
                .flat_map(|track| &mut track.devices),
        )
        .chain(&mut project.main_devices)
    {
        edit(device)?;
    }
    project.edit_automated_source_devices(edit)?;
    project.validate_device_automation()
}

fn assemble_lane(lane: &mut DeviceAutomation, placements: &[Placement]) -> Result<(), Invalid> {
    if !lane.points.is_empty() {
        lane.points = assemble_points(&lane.points, placements)?;
    }
    if !lane.touch.is_empty() {
        lane.touch = assemble_touches(&lane.touch, placements)?;
    }
    Ok(())
}

fn assemble_points(
    source: &[AutomationPoint],
    placements: &[Placement],
) -> Result<Vec<AutomationPoint>, Invalid> {
    let mut points = Vec::<AutomationPoint>::with_capacity(source.len());
    let mut previous_end = None;
    for placement in placements {
        let (value, outgoing) = base_sample(source, placement.start, false)
            .ok_or(Invalid("missing device automation section boundary"))?;
        if let Some(end) = previous_end
            && points
                .last()
                .is_some_and(|point| point.outgoing == AutomationInterpolation::Linear)
        {
            let (incoming, _) = base_sample(source, end, true).ok_or(Invalid(
                "missing incoming device automation section boundary",
            ))?;
            if incoming != value {
                points.push(AutomationPoint {
                    beat: placement.destination,
                    value: incoming,
                    outgoing: AutomationInterpolation::Step,
                });
            }
        }
        let first = source.partition_point(|point| point.beat <= placement.start);
        let authored = first > 0 && source[first - 1].beat == placement.start;
        if authored
            || outgoing != AutomationInterpolation::Step
            || points.last().is_none_or(|point| {
                point.outgoing != AutomationInterpolation::Step || point.value != value
            })
        {
            points.push(AutomationPoint {
                beat: placement.destination,
                value,
                outgoing,
            });
        }
        let last = placement.end.map_or(source.len(), |end| {
            source.partition_point(|point| point.beat < end)
        });
        let interior = &source[first..last];
        if points.len() + interior.len() > MAX_DEVICE_AUTOMATION_POINTS_PER_LANE {
            return Err(Invalid(
                "device automation section copy exceeds the lane point limit",
            ));
        }
        for point in interior {
            points.push(AutomationPoint {
                beat: placement.map(point.beat)?,
                ..*point
            });
        }
        previous_end = placement.end;
    }
    Ok(points)
}

fn assemble_touches(
    source: &[DeviceAutomationTouchSpan],
    placements: &[Placement],
) -> Result<Vec<DeviceAutomationTouchSpan>, Invalid> {
    let mut spans = Vec::with_capacity(source.len());
    for placement in placements {
        for span in source {
            let start = span.start.max(placement.start);
            let end = placement.end.map_or(span.end, |end| span.end.min(end));
            if start >= end {
                continue;
            }
            if spans.len() == MAX_DEVICE_AUTOMATION_TOUCH_SPANS {
                return Err(Invalid(
                    "device automation section copy exceeds the touch span limit",
                ));
            }
            let mut fragment = span.fragment(start, end)?;
            fragment.start = placement.map(start)?;
            fragment.end = placement.map(end)?;
            for step in &mut fragment.steps {
                step.beat = placement.map(step.beat)?;
            }
            spans.push(fragment);
        }
    }
    touch::validate(&spans)?;
    Ok(spans)
}
