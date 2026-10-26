//! One source-clock transformation, with independently placed local view laws.
use super::*;

fn frame_origin(clip: &Clip, edit: &AudioClipEdit) -> Result<i128, Invalid> {
    if let Some(view) = edit.content_view {
        i128::from(clip.start)
            .checked_add(view.base_start_delta)
            .and_then(|value| value.checked_sub(i128::from(view.base_view_start)))
            .and_then(|value| {
                value.checked_sub(i128::from(
                    view.base_placement
                        .map_or(0, |placement| placement.cycle_phase),
                ))
            })
            .ok_or(Invalid("alias source frame origin overflow"))
    } else {
        Ok(i128::from(clip.start)
            - i128::from(edit.view_start)
            - i128::from(edit.placement.map_or(0, |placement| placement.cycle_phase)))
    }
}
pub(crate) fn rebase_alias_content(
    target: &Clip,
    initiating: &Clip,
    content: &Arc<AudioClipContent>,
    command: &crate::Command,
    tempo: &TempoMap,
) -> Result<Clip, Invalid> {
    let mut result = target.clone();
    let before = initiating
        .audio_edit()
        .cloned()
        .unwrap_or_else(|| AudioClipEdit::capture(initiating));
    let mut edit = target
        .audio_edit()
        .cloned()
        .unwrap_or_else(|| AudioClipEdit::capture(target));
    match (before.domain.clock, content.clock) {
        (AudioDomainClock::Frames(_), AudioDomainClock::Beats(_)) => {
            let origin = frame_origin(initiating, &before)?;
            let old_origin = frame_origin(target, &edit)?;
            let old_fade_start = edit.domain.fade_start;
            let reference_start = if let Some(view) = edit.content_view {
                origin
                    .checked_add(i128::from(view.base_view_start))
                    .and_then(|value| {
                        value.checked_add(i128::from(
                            view.base_placement
                                .map_or(0, |placement| placement.cycle_phase),
                        ))
                    })
                    .and_then(|value| value.checked_sub(view.base_start_delta))
            } else {
                origin
                    .checked_add(i128::from(edit.view_start))
                    .and_then(|value| {
                        value.checked_add(i128::from(
                            edit.placement.map_or(0, |placement| placement.cycle_phase),
                        ))
                    })
            }
            .ok_or(Invalid("alias reference placement overflow"))?;
            edit.anchor_at_reference(target.start, reference_start, tempo)?;
            // Fade shapes stay in their original frame domain, independently of
            // the one shared conversion of event/root coordinates.
            let local_start_raw = edit.domain.fade_start;
            Arc::make_mut(&mut edit.domain).fade_projection = Some(AudioFadeProjection {
                tempo: tempo.clone(),
                origin_beat_raw: audio_projection_beat(
                    tempo,
                    old_origin
                        .checked_add(i128::from(old_fade_start))
                        .ok_or(Invalid("alias fade origin overflow"))?,
                )?,
                local_start_raw,
            });
        }
        (AudioDomainClock::Beats(_), AudioDomainClock::Frames(_)) => {
            return Err(Invalid(
                "alias content cannot flatten its shared musical source clock",
            ));
        }
        _ => (),
    }
    let ratio = match command {
        crate::Command::SetAudioClipSampleBpm { tempo: next, .. } => {
            Some(if let Some(previous) = before.domain.sample_bpm {
                (u64::from(next.micro_bpm()), u64::from(previous.micro_bpm()))
            } else {
                let desired = TempoMap::constant(tempo.sample_rate(), *next)?
                    .frame_to_beat(before.domain.source_length)?
                    .raw();
                let current = before
                    .domain
                    .warp
                    .as_ref()
                    .and_then(|warp| warp.markers.last())
                    .ok_or(Invalid("alias warp marker missing"))?
                    .beat
                    .raw();
                (desired, current)
            })
        }
        crate::Command::StretchAudioClipEdge { edge, frame, .. } => {
            let mut reference = before.clone();
            reference.anchor(initiating.start, tempo)?;
            let old_span = reference
                .placement
                .map_or(reference.view_end - reference.view_start, |placement| {
                    placement.outer_length.length()
                });
            let (start, end) = match edge {
                TrimEdge::Left => (
                    *frame,
                    initiating
                        .start
                        .checked_add(initiating.duration)
                        .ok_or(Invalid("alias stretch end overflow"))?,
                ),
                TrimEdge::Right => (initiating.start, *frame),
            };
            let new_span = tempo
                .frame_to_beat(end)?
                .raw()
                .checked_sub(tempo.frame_to_beat(start)?.raw())
                .ok_or(Invalid("alias stretch span invalid"))?;
            Some((new_span, old_span))
        }
        _ => None,
    };
    if let Some((numerator, denominator)) = ratio {
        super::stretch::retime(&mut edit, numerator, denominator, target.start, tempo)?;
    }
    if matches!(
        command,
        crate::Command::SetAudioClipMusical { enabled: false, .. }
    ) {
        freeze(&mut edit, tempo)?;
    } else if matches!(
        command,
        crate::Command::SetAudioClipMusical { enabled: true, .. }
            | crate::Command::SetAudioClipWarp {
                settings: Some(_),
                ..
            }
    ) && matches!(edit.timing, AudioClipTiming::Frozen { .. })
    {
        edit.anchor(target.start, tempo)?;
    }
    Arc::make_mut(&mut edit.domain).content = Arc::clone(content);
    result.edit = Some(Arc::new(edit));
    super::commands::project_clip(&mut result, tempo)?;
    Ok(result)
}
pub(crate) fn freeze(edit: &mut AudioClipEdit, tempo: &TempoMap) -> Result<(), Invalid> {
    if edit.reference_start_raw().is_none() || matches!(edit.timing, AudioClipTiming::Frozen { .. })
    {
        return Ok(());
    }
    if edit.domain.warp_enabled {
        return Err(Invalid("disable warp before frame anchoring"));
    }
    let reference_start_raw = edit.reference_for_processing_origin(edit.processing_origin()?)?;
    let map = edit.processing_projection.as_ref().map_or_else(
        || Arc::new(tempo.clone()),
        |projection| Arc::clone(&projection.tempo),
    );
    edit.timing = AudioClipTiming::Frozen {
        reference_start_raw,
        tempo: map,
    };
    edit.processing_projection = None;
    edit.placement_start_offset = 0;
    edit.placement_end_offset = 0;
    Ok(())
}
