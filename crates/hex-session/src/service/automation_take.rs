//! Session-facing completed capture queries delegate to the closed storage source.
//!
//! Storage owns semantic clock/file interpretation and bounded sequential spans so
//! the existing stream worker can consume the same producer without depending on
//! Session or accepting arbitrary page-producing closures. Session still owns live
//! source/output/transport admission and eventual canonical adoption. These APIs
//! perform control-side I/O; they grant no callback read or native resource lease.
pub use hex_storage::automation_draft::{
    AutomationSharedData, CompletedAutomationTake, MAX_PREVIEW_QUERIES, MAX_SCALAR_RANGE_FRAMES,
    MAX_SPANS_PER_STEP, PreparedAutomationTake, PreparedAutomationTakeDataLayout,
    ProjectedAutomationSpan, ProjectedSpanCursor, SpanReadProgress, SpanReadState,
};

/// Prepare one exact canonical splice from a validated, sealed native take.
/// The edit is committed later through the ordinary prepare-before-commit
/// transaction. On refusal the caller still owns the original sealed file.
pub fn prepare_native_take_adoption_on_control(
    session: &super::Session,
    document_base: Option<&std::path::Path>,
    take: &mut CompletedAutomationTake,
    expected: hex_storage::automation_draft::DraftProvenance,
) -> Result<hex_project::Command, String> {
    use hex_project::{
        BeatTime, DeviceAutomationTouchSpan, DeviceAutomationTouchStep,
        MAX_DEVICE_AUTOMATION_POINTS_PER_LANE, MAX_DEVICE_AUTOMATION_TOUCH_SPANS,
    };
    use hex_storage::automation_draft::{DraftTarget, MAX_APPEND_EVENTS};
    use std::sync::Arc;

    // Bound the one-shot control preparation separately from output density.
    // Large takes remain available to the original streamed source path.
    const MAX_ADOPTION_EVENTS: u64 = 65_536;
    let DraftTarget::Device { device, target } = take.target() else {
        return Err("sealed take does not target a native device".into());
    };
    if take.provenance() != expected {
        return Err("sealed take provenance differs from the completed gesture".into());
    }
    let project = super::recovery::project_for_save(session, document_base)?;
    if take.source() != &project {
        return Err("sealed take source differs from the current project; retain the draft".into());
    }
    hex_engine::native_parameter_input_target(&project, device, target)
        .map_err(|error| error.0.to_owned())?;
    if take.event_count() > MAX_ADOPTION_EVENTS {
        return Err(
            "sealed take exceeds the bounded native adoption work; retain the draft".into(),
        );
    }

    let map = Arc::new(project.tempo_map().clone());
    let [first, second] = take.projected_coverage(&map)?;
    let first = first
        .or_else(|| second.clone())
        .ok_or("sealed take has no covered time")?;
    let end = second.map_or(first.end, |range| range.end.max(first.end));
    let mut cursor = take.begin_projected_spans_on_control(Arc::clone(&map), first.start..end)?;
    let mut output = [None; MAX_SPANS_PER_STEP];
    let mut spans = Vec::<DeviceAutomationTouchSpan>::new();
    let mut steps = 0usize;
    let beat_at = |frame: u64| -> Result<BeatTime, String> {
        let beat = map
            .frame_to_beat(frame)
            .map_err(|error| error.0.to_owned())?;
        if map.beat_to_frame(beat).ok() != Some(frame) {
            return Err("sealed take boundary cannot be placed at its exact sample".into());
        }
        Ok(beat)
    };
    loop {
        let progress = take.read_projected_spans_on_control(
            &map,
            &mut cursor,
            MAX_APPEND_EVENTS,
            &mut output,
        )?;
        for row in output.iter().take(progress.written).flatten() {
            let start = beat_at(row.start)?;
            let end = beat_at(row.end)?;
            if start >= end {
                return Err("sealed take span has no exact musical interval".into());
            }
            if spans.last().is_none_or(|span| span.end != start) {
                if spans.len() == MAX_DEVICE_AUTOMATION_TOUCH_SPANS {
                    return Err(
                        "sealed take exceeds the touch span budget; retain the draft".into(),
                    );
                }
                spans
                    .try_reserve(1)
                    .map_err(|_| "could not reserve touch spans")?;
                spans.push(DeviceAutomationTouchSpan {
                    start,
                    end,
                    steps: Vec::new(),
                });
            }
            let span = spans.last_mut().ok_or("missing prepared touch span")?;
            span.end = end;
            if span.steps.last().is_none_or(|step| step.value != row.value) {
                if steps == MAX_DEVICE_AUTOMATION_POINTS_PER_LANE {
                    return Err(
                        "sealed take exceeds the touch point budget; retain the draft".into(),
                    );
                }
                span.steps
                    .try_reserve(1)
                    .map_err(|_| "could not reserve touch steps")?;
                span.steps.push(DeviceAutomationTouchStep {
                    beat: start,
                    value: row.value,
                });
                steps += 1;
            }
        }
        if progress.state == SpanReadState::Finished {
            break;
        }
    }
    if spans.is_empty() {
        return Err("sealed take has no covered time".into());
    }
    Ok(hex_project::Command::SpliceDeviceAutomationTouch {
        device,
        target,
        spans,
    })
}

#[cfg(test)]
use hex_project::{LoopRegion, Project};
#[cfg(test)]
use hex_storage::automation_draft::{
    AppliedEvent, CompletedDraft, DraftClock, DraftProvenance, DraftReadLimits, DraftTarget,
    MAX_APPEND_EVENTS,
};
#[cfg(test)]
use std::path::Path;
#[cfg(test)]
mod tests;
