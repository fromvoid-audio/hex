//! Reassemble canonical device automation from consecutive half-open source
//! slices laid out in destination order. Section copies may repeat a source
//! slice. A two-point seam gives each adjacent curve its original limit without
//! inventing a one-tick ramp. Touch overlays retain their separately held values.
use super::{
    AutomationInterpolation, AutomationPoint, AutomationReadSelection, AutomationValue,
    DeviceAutomation, DeviceAutomationTarget, DeviceAutomationTouchSpan,
    MAX_DEVICE_AUTOMATION_LANES, MAX_DEVICE_AUTOMATION_POINTS_PER_LANE,
    MAX_DEVICE_AUTOMATION_TOUCH_SPANS, base_sample, touch,
};
use crate::{BeatTime, Device, DeviceId, Invalid, Project};
use std::collections::{BTreeMap, BTreeSet};

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
    assemble(
        project,
        &[
            (BeatTime::ZERO, Some(insertion)),
            (start, Some(end)),
            (insertion, None),
        ],
    )
}

/// Cut the half-open section interval and ripple the surviving envelopes.
pub(crate) fn delete_section(
    project: &mut Project,
    start: BeatTime,
    end: BeatTime,
) -> Result<(), Invalid> {
    assemble(project, &[(BeatTime::ZERO, Some(start)), (end, None)])
}

fn assemble(project: &mut Project, slices: &[(BeatTime, Option<BeatTime>)]) -> Result<(), Invalid> {
    if !project.has_device_automation() {
        return Ok(());
    }
    let mut next = BeatTime::ZERO;
    let mut placements = Vec::with_capacity(slices.len());
    for &(start, end) in slices {
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
        // Deleting an entire touch-only envelope leaves no lane to read.
        device
            .automation
            .retain(|lane| !lane.points.is_empty() || !lane.touch.is_empty());
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
        for point in &source[first..last] {
            points.push(AutomationPoint {
                beat: placement.map(point.beat)?,
                ..*point
            });
        }
        previous_end = placement.end;
    }
    // The curve holds its first value backwards. An initial synthetic hold
    // that repeats that value spends capacity without changing the envelope.
    let first_start = placements.first().map(|placement| placement.start);
    if let [first, second, ..] = points.as_slice()
        && first_start.is_some_and(|start| !source.iter().any(|point| point.beat == start))
        && first.outgoing == AutomationInterpolation::Step
        && first.value == second.value
    {
        points.remove(0);
    }
    if points.len() > MAX_DEVICE_AUTOMATION_POINTS_PER_LANE {
        return Err(Invalid(
            "device automation section edit exceeds the lane point limit",
        ));
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
                    "device automation section edit exceeds the touch span limit",
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

#[derive(Clone, Debug)]
struct CapturedPoints {
    start_value: AutomationValue,
    start_outgoing: AutomationInterpolation,
    start_authored: bool,
    end_incoming: AutomationValue,
    interior: Vec<AutomationPoint>,
}

#[derive(Clone, Debug)]
struct CapturedLane {
    read_selection: AutomationReadSelection,
    target: DeviceAutomationTarget,
    vst3_class: Option<[u8; 16]>,
    points: Option<CapturedPoints>,
    touch: Vec<DeviceAutomationTouchSpan>,
}

/// Finite device-automation slice captured from a single song section, rebased
/// to `[0, length)` and keyed by stable `DeviceId`.
#[derive(Clone, Debug, Default)]
pub(crate) struct CapturedDeviceAutomation {
    devices: BTreeMap<DeviceId, Vec<CapturedLane>>,
}

impl CapturedDeviceAutomation {
    pub(crate) fn is_empty(&self) -> bool {
        self.devices.is_empty()
    }
}

pub(crate) fn capture_section(
    project: &Project,
    start: BeatTime,
    end: BeatTime,
) -> Result<CapturedDeviceAutomation, Invalid> {
    if start >= end {
        return Err(Invalid(
            "device automation section capture requires a positive range",
        ));
    }
    let mut devices = BTreeMap::new();
    let mut capture_device = |device: &Device| -> Result<(), Invalid> {
        if device.automation.is_empty() {
            return Ok(());
        }
        let mut lanes = Vec::new();
        for lane in &device.automation {
            let points = if lane.points.is_empty() {
                None
            } else {
                let (start_value, start_outgoing) = base_sample(&lane.points, start, false)
                    .ok_or(Invalid("missing device automation capture start"))?;
                let (end_incoming, _) = base_sample(&lane.points, end, true)
                    .ok_or(Invalid("missing device automation capture end"))?;
                let start_authored = lane.points.iter().any(|point| point.beat == start);
                let first = lane.points.partition_point(|point| point.beat <= start);
                let last = lane.points.partition_point(|point| point.beat < end);
                let mut interior = Vec::new();
                interior
                    .try_reserve_exact(last.saturating_sub(first))
                    .map_err(|_| Invalid("could not reserve device automation section capture"))?;
                for point in &lane.points[first..last] {
                    interior.push(AutomationPoint {
                        beat: BeatTime::from_raw(
                            point
                                .beat
                                .raw()
                                .checked_sub(start.raw())
                                .ok_or(Invalid("device automation capture underflow"))?,
                        ),
                        ..*point
                    });
                }
                Some(CapturedPoints {
                    start_value,
                    start_outgoing,
                    start_authored,
                    end_incoming,
                    interior,
                })
            };
            let mut touch_spans = Vec::new();
            for span in &lane.touch {
                let a = span.start.max(start);
                let b = span.end.min(end);
                if a >= b {
                    continue;
                }
                let mut fragment = span.fragment(a, b)?;
                fragment.start = BeatTime::from_raw(a.raw() - start.raw());
                fragment.end = BeatTime::from_raw(b.raw() - start.raw());
                for step in &mut fragment.steps {
                    step.beat = BeatTime::from_raw(
                        step.beat
                            .raw()
                            .checked_sub(start.raw())
                            .ok_or(Invalid("device automation touch capture underflow"))?,
                    );
                }
                touch_spans
                    .try_reserve(1)
                    .map_err(|_| Invalid("could not reserve device automation touch capture"))?;
                touch_spans.push(fragment);
            }
            if points.is_some() || !touch_spans.is_empty() {
                lanes
                    .try_reserve(1)
                    .map_err(|_| Invalid("could not reserve captured device automation lane"))?;
                lanes.push(CapturedLane {
                    read_selection: lane.read_selection,
                    target: lane.target,
                    vst3_class: lane.vst3_class,
                    points,
                    touch: touch_spans,
                });
            }
        }
        if !lanes.is_empty() {
            devices.insert(device.id(), lanes);
        }
        Ok(())
    };
    for device in project
        .tracks
        .iter()
        .flat_map(|track| &track.devices)
        .chain(project.returns.iter().flat_map(|track| &track.devices))
        .chain(&project.main_devices)
        .chain(
            project
                .native_sources
                .iter()
                .flat_map(|source| source.devices()),
        )
    {
        capture_device(device)?;
    }
    Ok(CapturedDeviceAutomation { devices })
}

pub(crate) fn paste_section(
    project: &mut Project,
    at: BeatTime,
    length: BeatTime,
    captured: &CapturedDeviceAutomation,
) -> Result<(), Invalid> {
    if !project.has_device_automation() && captured.is_empty() {
        return Ok(());
    }
    let end = BeatTime::from_raw(
        at.raw()
            .checked_add(length.raw())
            .ok_or(Invalid("device automation section paste overflow"))?,
    );
    let captured_ids: BTreeSet<DeviceId> = captured.devices.keys().copied().collect();
    let edit = |device: &mut Device| -> Result<(), Invalid> {
        let device_captured = captured.devices.get(&device.id());
        let mut matched = vec![false; device_captured.map_or(0, Vec::len)];
        for lane in &mut device.automation {
            let captured_lane = device_captured.and_then(|lanes| {
                lanes
                    .iter()
                    .enumerate()
                    .find(|(_, item)| {
                        item.target == lane.target && item.vst3_class == lane.vst3_class
                    })
                    .map(|(index, item)| {
                        matched[index] = true;
                        item
                    })
            });
            if !lane.points.is_empty() || captured_lane.is_some_and(|item| item.points.is_some()) {
                lane.points = splice_pasted_points(
                    &lane.points,
                    at,
                    end,
                    length,
                    captured_lane.and_then(|item| item.points.as_ref()),
                )?;
            }
            if !lane.touch.is_empty() || captured_lane.is_some_and(|item| !item.touch.is_empty()) {
                lane.touch = splice_pasted_touches(
                    &lane.touch,
                    at,
                    length,
                    captured_lane.map_or(&[], |item| item.touch.as_slice()),
                )?;
            }
        }
        if let Some(lanes) = device_captured {
            for (index, item) in lanes.iter().enumerate() {
                if matched[index] {
                    continue;
                }
                if device.automation.len() >= MAX_DEVICE_AUTOMATION_LANES {
                    return Err(Invalid("device automation lane limit reached"));
                }
                let points = match &item.points {
                    Some(cp) => splice_pasted_points(&[], at, end, length, Some(cp))?,
                    None => Vec::new(),
                };
                let touch = if item.touch.is_empty() {
                    Vec::new()
                } else {
                    splice_pasted_touches(&[], at, length, &item.touch)?
                };
                if !points.is_empty() || !touch.is_empty() {
                    device.automation.push(DeviceAutomation {
                        read_selection: item.read_selection,
                        target: item.target,
                        vst3_class: item.vst3_class,
                        points,
                        touch,
                    });
                }
            }
        }
        device
            .automation
            .retain(|lane| !lane.points.is_empty() || !lane.touch.is_empty());
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
    project.edit_automated_or_captured_source_devices(&captured_ids, edit)?;
    project.validate_device_automation()
}

fn splice_pasted_points(
    source: &[AutomationPoint],
    at: BeatTime,
    end: BeatTime,
    length: BeatTime,
    captured: Option<&CapturedPoints>,
) -> Result<Vec<AutomationPoint>, Invalid> {
    let mut points = Vec::new();
    let reserve_len = source
        .len()
        .saturating_add(captured.map_or(2, |cp| cp.interior.len().saturating_add(4)));
    points
        .try_reserve(reserve_len)
        .map_err(|_| Invalid("could not reserve device automation paste points"))?;

    // 1. Prefix [0, at) from existing lane.
    if !source.is_empty() && at > BeatTime::ZERO {
        let (start_val, start_out) = base_sample(source, BeatTime::ZERO, false)
            .ok_or(Invalid("missing device automation paste prefix start"))?;
        let first = source.partition_point(|point| point.beat <= BeatTime::ZERO);
        let authored = first > 0 && source[first - 1].beat == BeatTime::ZERO;
        if authored || start_out != AutomationInterpolation::Step || points.is_empty() {
            points.push(AutomationPoint {
                beat: BeatTime::ZERO,
                value: start_val,
                outgoing: start_out,
            });
        }
        let last = source.partition_point(|point| point.beat < at);
        points.extend_from_slice(&source[first..last]);
    }

    // 2. Inserted slice [at, end).
    let inserted_end_incoming = if let Some(cp) = captured {
        if !source.is_empty()
            && at > BeatTime::ZERO
            && points
                .last()
                .is_some_and(|point| point.outgoing == AutomationInterpolation::Linear)
        {
            let (incoming, _) = base_sample(source, at, true)
                .ok_or(Invalid("missing device automation prefix boundary"))?;
            if incoming != cp.start_value {
                points.push(AutomationPoint {
                    beat: at,
                    value: incoming,
                    outgoing: AutomationInterpolation::Step,
                });
            }
        }
        if cp.start_authored
            || cp.start_outgoing != AutomationInterpolation::Step
            || points.last().is_none_or(|point| {
                point.outgoing != AutomationInterpolation::Step || point.value != cp.start_value
            })
        {
            points.push(AutomationPoint {
                beat: at,
                value: cp.start_value,
                outgoing: cp.start_outgoing,
            });
        }
        for point in &cp.interior {
            let beat = BeatTime::from_raw(
                at.raw()
                    .checked_add(point.beat.raw())
                    .ok_or(Invalid("device automation paste interior overflow"))?,
            );
            points.push(AutomationPoint { beat, ..*point });
        }
        Some(cp.end_incoming)
    } else if !source.is_empty() {
        // Hold the prevailing value across the inserted interval.
        let (held, _) = base_sample(source, at, true)
            .ok_or(Invalid("missing device automation paste hold boundary"))?;
        if points.last().is_none_or(|point| {
            point.outgoing != AutomationInterpolation::Step || point.value != held
        }) {
            points.push(AutomationPoint {
                beat: at,
                value: held,
                outgoing: AutomationInterpolation::Step,
            });
        }
        Some(held)
    } else {
        None
    };

    // 3. Suffix [at, ∞) from existing lane, shifted to [end, ∞).
    if !source.is_empty() {
        let (suffix_val, suffix_out) = base_sample(source, at, false)
            .ok_or(Invalid("missing device automation paste suffix start"))?;
        if let Some(incoming) = inserted_end_incoming
            && points
                .last()
                .is_some_and(|point| point.outgoing == AutomationInterpolation::Linear)
            && incoming != suffix_val
        {
            points.push(AutomationPoint {
                beat: end,
                value: incoming,
                outgoing: AutomationInterpolation::Step,
            });
        }
        let first = source.partition_point(|point| point.beat <= at);
        let authored = first > 0 && source[first - 1].beat == at;
        if authored
            || suffix_out != AutomationInterpolation::Step
            || points.last().is_none_or(|point| {
                point.outgoing != AutomationInterpolation::Step || point.value != suffix_val
            })
        {
            points.push(AutomationPoint {
                beat: end,
                value: suffix_val,
                outgoing: suffix_out,
            });
        }
        for point in &source[first..] {
            let beat = BeatTime::from_raw(
                point
                    .beat
                    .raw()
                    .checked_add(length.raw())
                    .ok_or(Invalid("device automation paste suffix overflow"))?,
            );
            points.push(AutomationPoint { beat, ..*point });
        }
    } else if let Some(incoming) = inserted_end_incoming
        && points
            .last()
            .is_some_and(|point| point.outgoing == AutomationInterpolation::Linear)
    {
        points.push(AutomationPoint {
            beat: end,
            value: incoming,
            outgoing: AutomationInterpolation::Step,
        });
    }

    // Same synthetic-initial-hold trim as assemble_points.
    if !source.is_empty()
        && at > BeatTime::ZERO
        && let [first, second, ..] = points.as_slice()
        && !source.iter().any(|point| point.beat == BeatTime::ZERO)
        && first.outgoing == AutomationInterpolation::Step
        && first.value == second.value
    {
        points.remove(0);
    }
    if points.len() > MAX_DEVICE_AUTOMATION_POINTS_PER_LANE {
        return Err(Invalid(
            "device automation section edit exceeds the lane point limit",
        ));
    }
    Ok(points)
}

fn splice_pasted_touches(
    source: &[DeviceAutomationTouchSpan],
    at: BeatTime,
    length: BeatTime,
    captured: &[DeviceAutomationTouchSpan],
) -> Result<Vec<DeviceAutomationTouchSpan>, Invalid> {
    let mut spans = Vec::new();
    let push_span = |spans: &mut Vec<DeviceAutomationTouchSpan>,
                     span: DeviceAutomationTouchSpan|
     -> Result<(), Invalid> {
        if spans.len() >= MAX_DEVICE_AUTOMATION_TOUCH_SPANS {
            return Err(Invalid(
                "device automation section edit exceeds the touch span limit",
            ));
        }
        spans
            .try_reserve(1)
            .map_err(|_| Invalid("could not reserve device automation touch paste"))?;
        spans.push(span);
        Ok(())
    };
    // 1. Prefix [0, at)
    for span in source {
        let a = span.start;
        let b = span.end.min(at);
        if a < b {
            push_span(&mut spans, span.fragment(a, b)?)?;
        }
    }
    // 2. Inserted [at, at + length)
    for span in captured {
        let mut shifted = span.clone();
        shifted.start = BeatTime::from_raw(
            at.raw()
                .checked_add(span.start.raw())
                .ok_or(Invalid("device automation touch paste overflow"))?,
        );
        shifted.end = BeatTime::from_raw(
            at.raw()
                .checked_add(span.end.raw())
                .ok_or(Invalid("device automation touch paste overflow"))?,
        );
        for step in &mut shifted.steps {
            step.beat = BeatTime::from_raw(
                at.raw()
                    .checked_add(step.beat.raw())
                    .ok_or(Invalid("device automation touch paste step overflow"))?,
            );
        }
        push_span(&mut spans, shifted)?;
    }
    // 3. Suffix [at, ∞) shifted by +length
    for span in source {
        let a = span.start.max(at);
        let b = span.end;
        if a < b {
            let mut shifted = span.fragment(a, b)?;
            shifted.start = BeatTime::from_raw(
                a.raw()
                    .checked_add(length.raw())
                    .ok_or(Invalid("device automation touch suffix overflow"))?,
            );
            shifted.end = BeatTime::from_raw(
                b.raw()
                    .checked_add(length.raw())
                    .ok_or(Invalid("device automation touch suffix overflow"))?,
            );
            for step in &mut shifted.steps {
                step.beat = BeatTime::from_raw(
                    step.beat
                        .raw()
                        .checked_add(length.raw())
                        .ok_or(Invalid("device automation touch suffix step overflow"))?,
                );
            }
            push_span(&mut spans, shifted)?;
        }
    }
    touch::validate(&spans)?;
    Ok(spans)
}
