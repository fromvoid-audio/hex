//! Bounded sealed device takes become one canonical history command. Storage
//! owns projection/clock semantics; Project owns the atomic splice and history.
//! This control-side conversion grants no SDK, live-source or callback authority.
use super::{CompletedAutomationTake, MAX_SPANS_PER_STEP, SpanReadState};

/// Prepare one exact canonical splice from a validated, sealed device take.
/// The edit is committed later through the ordinary prepare-before-commit
/// transaction. On refusal the caller still owns the original sealed file.
pub fn prepare_device_take_adoption_on_control(
    session: &crate::Session,
    document_base: Option<&std::path::Path>,
    take: &mut CompletedAutomationTake,
    expected: hex_storage::automation_draft::DraftProvenance,
) -> Result<hex_project::Command, String> {
    use hex_project::{
        BeatTime, DeviceAutomationTouchSpan, DeviceAutomationTouchStep,
        MAX_DEVICE_AUTOMATION_TOUCH_SPANS, MAX_DEVICE_AUTOMATION_TOUCH_STEPS_PER_LANE,
    };
    use hex_storage::automation_draft::{DraftTarget, MAX_APPEND_EVENTS};
    use std::sync::Arc;

    // Bound the one-shot control preparation separately from output density.
    // Large takes remain available to the original streamed source path.
    const MAX_ADOPTION_EVENTS: u64 = 65_536;
    let DraftTarget::Device { device, target } = take.target() else {
        return Err("sealed take does not target a device".into());
    };
    if take.provenance() != expected {
        return Err("sealed take provenance differs from the completed gesture".into());
    }
    let project = super::super::recovery::project_for_save(session, document_base)?;
    if take.source() != &project {
        return Err("sealed take source differs from the current project; retain the draft".into());
    }
    let vst3_class = if matches!(target, hex_project::DeviceAutomationTarget::Vst3 { .. }) {
        // The source retains class identity; SDK capabilities and publication
        // remain the ordinary prepare-before-commit consumer's responsibility.
        take.target()
            .vst3_class_in_source(&project)
            .map_err(|error| error.to_string())?
    } else {
        hex_engine::native_parameter_input_target(&project, device, target)
            .map_err(|error| error.0.to_owned())?;
        None
    };
    if take.event_count() > MAX_ADOPTION_EVENTS {
        return Err(
            "sealed take exceeds the bounded device adoption work; retain the draft".into(),
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
                if steps == MAX_DEVICE_AUTOMATION_TOUCH_STEPS_PER_LANE {
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
        vst3_class,
        device,
        target,
        spans,
    })
}

#[cfg(test)]
mod tests;
