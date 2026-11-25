//! Mutable instrument chunk rendering, console MIDI onset collection, and instrument clock.
use super::*;

pub(super) struct MutableInstrumentChunkContext<'a, T: ?Sized> {
    pub(super) sources: Option<crate::native_device_resources::SamplerSourcesRef<'a>>,
    pub(super) revision: &'a PreparedRevision,
    pub(super) midi: crate::session_inventory::EffectiveSessionMidi<'a>,
    pub(super) routes: &'a [super::TrackRoute],
    pub(super) runtimes: &'a mut [Option<MutableInstrumentTrackRuntime>],
    pub(super) onsets: &'a mut MutableOnsetBuffer,
    pub(super) suppressions: &'a [MidiOccurrenceSuppression],
    pub(super) events: &'a mut Vec<hex_lattice::NativeInstrumentEvent>,
    pub(super) spans: &'a mut MidiSpanBuffer,
    pub(super) performance_frame: u64,
    pub(super) arrangement_frame: u64,
    pub(super) arrangement_loop_pass: u64,
    pub(super) transient: Option<hex_lattice::NativeTransientInput>,
    pub(super) chase_at_start: bool,
    pub(super) interactive_onset_floor: Option<crate::midi_compile::InteractiveOnsetFloor>,
    pub(super) pending_shared_chase: Option<super::PendingSharedChase>,
    pub(super) loop_region: Option<hex_project::LoopRegion>,
    pub(super) timing: &'a T,
}

/// Gather canonical Console note onsets for an already selected MIDI route.
/// Both native and foreign generators consume the same admitted clip template.
pub(in crate::session_playback) fn collect_console_midi_onsets<T: MidiTiming + ?Sized>(
    midi: &crate::session_inventory::EffectiveSessionMidi<'_>,
    route: TrackRoute,
    timing: &T,
    start: u64,
    end: u64,
    spans: &mut MidiSpanBuffer,
    onsets: &mut MutableOnsetBuffer,
) -> Result<(), SessionPlaybackError> {
    let RouteOwner::Session { clip, origin, .. } = route.owner else {
        return Err(SessionPlaybackError::InvalidCommand);
    };
    let prepared = midi
        .midi_clip_for_track(clip, route.id)
        .ok_or(SessionPlaybackError::UnknownClip)?;
    if prepared.track != route.id {
        return Err(SessionPlaybackError::ClipTrackMismatch);
    }
    if !prepared.instrument_enabled() {
        return Ok(());
    }
    prepared
        .instrument_template()
        .query_with_origin_and_note_chase(
            origin,
            timing,
            (start, end),
            spans,
            route.owner.note_chase(),
        )
        .map_err(SessionPlaybackError::Midi)?;
    for span in spans.spans() {
        onsets
            .push(crate::mutable_instrument::MutableNoteOnset {
                key: crate::mutable_instrument::MutableOccurrenceKey::new(
                    prepared.track,
                    span.clip,
                    span.note,
                    span.iteration,
                    route.current_incarnation,
                ),
                onset_beat: span.onset_beat,
                gate_end_beat: span.gate_end_beat,
                source_start: span.start_frame,
                source_gate_end: span.gate_end_frame,
                pitch: span.pitch,
                channel: span.channel,
                velocity: span.velocity,
                expression: span.expression,
                contributor_fade_in: false,
            })
            .map_err(|_| SessionPlaybackError::MutableInstrumentEvents)?;
    }
    Ok(())
}

pub(super) fn render_mutable_instrument_chunk<
    T: crate::prepared_tempo::MidiTempoBreaks + ?Sized,
>(
    mut context: MutableInstrumentChunkContext<'_, T>,
    index: usize,
    output: &mut [Frame],
    processing: Option<&mut crate::native_parameter_input::Processing>,
) -> Result<(), SessionPlaybackError> {
    let origin = (
        context.arrangement_frame,
        context.arrangement_loop_pass,
        context.performance_frame,
    );
    context.transient = processing.as_deref().and_then(|processing| {
        let track = context.revision.arrangement().tracks().get(index)?;
        if output.is_empty()
            || !track.receives_source_audio()
            || context
                .runtimes
                .get(index)
                .and_then(Option::as_ref)
                .is_none()
        {
            return None;
        }
        processing.for_source(track.source_id(), track.instrument_id)
    });
    let applied = context.transient.is_some();
    render_mutable_instrument_inner(context, index, output)?;
    if applied && let Some(processing) = processing {
        processing
            .source_applied(origin.0, origin.1, origin.2)
            .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
    }
    Ok(())
}

pub(super) fn render_mutable_instrument_inner<
    T: crate::prepared_tempo::MidiTempoBreaks + ?Sized,
>(
    context: MutableInstrumentChunkContext<'_, T>,
    index: usize,
    output: &mut [Frame],
) -> Result<(), SessionPlaybackError> {
    if context.revision.source_routing().is_shared(index) {
        return shared_source::render_mutable_source_chunk(context, index, output);
    }
    let MutableInstrumentChunkContext {
        revision,
        midi,
        sources,
        routes,
        runtimes,
        onsets,
        suppressions,
        events,
        spans,
        performance_frame,
        arrangement_frame,
        arrangement_loop_pass,
        transient,
        chase_at_start,
        interactive_onset_floor,
        pending_shared_chase: _,
        loop_region,
        timing,
    } = context;
    let Some(runtime) = runtimes.get_mut(index).and_then(Option::as_mut) else {
        return Ok(());
    };
    let chase_at_start = chase_at_start && !runtime.retained_at_latency_cut(performance_frame);
    let track = revision
        .arrangement()
        .tracks()
        .get(index)
        .ok_or(SessionPlaybackError::UnknownTrack)?;
    let track_id = track
        .canonical_track_id()
        .ok_or(SessionPlaybackError::RevisionTrackMismatch)?;
    let source_end = performance_frame
        .checked_add(
            u64::try_from(output.len()).map_err(|_| SessionPlaybackError::ArithmeticOverflow)?,
        )
        .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
    onsets.clear();
    let midi_route = effective_midi_route(revision, routes, index)?;
    match midi_route.owner {
        RouteOwner::Arrangement => {
            let mut event_error = false;
            let arrangement_end = arrangement_frame
                .checked_add(
                    u64::try_from(output.len())
                        .map_err(|_| SessionPlaybackError::ArithmeticOverflow)?,
                )
                .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
            for expr in track.expression_events_in_range(arrangement_frame, arrangement_end) {
                event_error |= onsets.push_expression(*expr).is_err();
            }
            track
                .for_each_mutable_midi_span(arrangement_frame, arrangement_end, |span| {
                    if !span.admitted_at_interactive_start(interactive_onset_floor) {
                        return;
                    }
                    let key = crate::mutable_instrument::MutableOccurrenceKey::new(
                        track_id,
                        span.clip,
                        span.note,
                        span.iteration,
                        arrangement_loop_pass,
                    );
                    // A forward loop edit on pass > 0 can reach `region.start()`
                    // linearly while an already-started span from this pass is
                    // active; skip wrap-chase for any key already owned on this pass.
                    let is_wrap_chase = arrangement_loop_pass > 0
                        && loop_region.is_some_and(|region| {
                            arrangement_frame == region.start()
                                && span.start < region.start()
                                && span.gate_end > region.start()
                        })
                        && !runtime.proofs().any(|proof| proof.signature.key() == key);
                    let is_start_chase = chase_at_start
                        && span.start < arrangement_frame
                        && span.gate_end > arrangement_frame;
                    let source_start = if is_wrap_chase || is_start_chase {
                        arrangement_frame
                    } else {
                        span.start
                    };
                    let source_gate_end = if is_wrap_chase {
                        loop_region
                            .map(|region| span.gate_end.min(region.end()))
                            .ok_or(())
                    } else {
                        Ok(loop_region
                            .map_or(span.gate_end, |region| span.gate_end.min(region.end())))
                    };
                    let Ok(source_gate_end) = source_gate_end else {
                        event_error = true;
                        return;
                    };
                    event_error |= onsets
                        .push(crate::mutable_instrument::MutableNoteOnset {
                            key,
                            onset_beat: span.onset_beat,
                            gate_end_beat: span.gate_end_beat,
                            source_start,
                            source_gate_end,
                            pitch: span.pitch,
                            channel: span.channel,
                            velocity: span.velocity,
                            expression: span.expression,
                            contributor_fade_in: false,
                        })
                        .is_err();
                })
                .map_err(|_| SessionPlaybackError::MutableInstrumentEvents)?;
            if event_error {
                return Err(SessionPlaybackError::MutableInstrumentEvents);
            }
        }
        RouteOwner::Session { .. } => {
            collect_console_midi_onsets(
                &midi,
                midi_route,
                timing,
                performance_frame,
                source_end,
                spans,
                onsets,
            )?;
        }
        RouteOwner::SessionAudio { .. } | RouteOwner::Silence => {}
    }
    let scoped = super::mutable_setup::resolve_track_sampler_resources(sources, track_id)?;
    let resources = super::mutable_setup::as_device_resources(&scoped);
    let source_start = match midi_route.owner {
        RouteOwner::Arrangement => arrangement_frame,
        _ => performance_frame,
    };
    let arrangement_volumes = track.instrument.as_ref().and_then(|i| i.volumes.as_ref());
    let console = if let RouteOwner::Session { clip, origin, .. } = midi_route.owner {
        midi.midi_clip_for_track(clip, track_id)
            .map(|prepared| (clip, origin, prepared))
    } else {
        None
    };
    let has_program = match midi_route.owner {
        RouteOwner::Arrangement => arrangement_volumes.is_some(),
        RouteOwner::Session { .. } => console
            .as_ref()
            .is_some_and(|(_, _, prepared)| prepared.volume_program().is_some()),
        _ => false,
    };
    let parameter_context = instrument_automation::SourceAutomation::new(
        revision,
        index,
        super::mutable_setup::running_automation_origin(
            arrangement_frame,
            performance_frame,
            arrangement_loop_pass,
        ),
        output.len(),
    )?;
    let parameter_block = parameter_context.block_with_input(transient)?;
    let parameter_values = parameter_block
        .as_ref()
        .map(|values| values as &dyn hex_lattice::NativeParameterAutomation);
    let clock = InstrumentClock {
        timing,
        start: performance_frame,
    };
    if !has_program && !runtime.has_volume_gain() {
        return runtime
            .process_segment_with_resources(
                track_id,
                performance_frame,
                source_start,
                output,
                onsets,
                suppressions,
                events,
                Some(&clock),
                resources,
                parameter_values,
            )
            .map_err(crate::mutable_instrument::MutableInstrumentRuntimeError::render_error);
    }
    let console_program = console
        .as_ref()
        .and_then(|(_, origin, prepared)| prepared.volume_program().map(|p| (*origin, p)));
    let (console_historical, console_endpoint) =
        super::mutable_setup::console_volume_bounds(console_program, timing)
            .map_err(|()| SessionPlaybackError::MutableInstrumentEvents)?;
    let mut lookup = |signature: crate::mutable_instrument::MutableOccurrenceSignature,
                      performance: u64,
                      frames: usize,
                      _retaining_muted: bool| {
        use crate::midi_clip_volume::VolumePlacement;
        use crate::mutable_instrument::{MutableInstrumentRuntimeError as Error, VolumeSlice};
        let key = signature.key();
        let unity = super::mutable_setup::unity_volume_slice;
        match midi_route.owner {
            RouteOwner::Arrangement => {
                if key.track != track_id || key.loop_pass != arrangement_loop_pass {
                    return Ok(None);
                }
                // CURRENT deliberately omits ordinary mutable source descriptor
                // owners. Its existing admitted note interval index is the
                // authority: query at this exact authored onset, bounded by the
                // compiled voice-density law, never scan canonical clips/notes.
                // A missing/edited source keeps the outgoing scalar; an exact
                // source with an empty curve explicitly restores unity.
                let (onset, gate, pitch, channel, velocity) = signature.details();
                let first = revision
                    .session()
                    .timing()
                    .beat_to_frame(onset)
                    .map_err(|_| Error::InvalidSourceSpan)?;
                let end = first.checked_add(1).ok_or(Error::Arithmetic)?;
                let mut found = false;
                track
                    .for_each_mutable_midi_span(first, end, |span| {
                        found |= span.clip == key.clip
                            && span.note == key.note
                            && span.iteration == key.iteration
                            && span.onset_beat == onset
                            && span.gate_end_beat == gate
                            && span.pitch == pitch
                            && span.channel == channel
                            && span.velocity == velocity;
                    })
                    .map_err(|_| Error::InvalidSourceSpan)?;
                if !found {
                    return Ok(None);
                }
                let at = performance
                    .checked_sub(performance_frame)
                    .and_then(|offset| arrangement_frame.checked_add(offset))
                    .ok_or(Error::Arithmetic)?;
                arrangement_volumes.and_then(|v| v.clip(key.clip)).map_or(
                    Ok(Some(unity(at))),
                    |row| {
                        let Some(endpoint) = row.endpoint else {
                            return Ok(None);
                        };
                        if at >= endpoint.frame {
                            return Ok(None);
                        }
                        // Arrangement coordinates are content frames. The Console
                        // clock may have an absolute rebase floor after a wrap.
                        row.segment_or_hold(revision.session().timing(), at, frames)
                            .map(|ramp| {
                                Some(VolumeSlice {
                                    at,
                                    ramp,
                                    terminal: Some(endpoint.frame),
                                })
                            })
                            .map_err(|_| Error::InvalidSourceSpan)
                    },
                )
            }
            RouteOwner::Session { clip, origin, .. } => {
                if key.clip != clip || key.loop_pass != routes[index].current_incarnation {
                    return Ok(None);
                }
                let Some(program) = console
                    .as_ref()
                    .and_then(|(_, _, prepared)| prepared.volume_program())
                else {
                    return Ok(Some(unity(performance)));
                };
                if console_historical || console_endpoint.is_some_and(|e| performance >= e.frame) {
                    return Ok(None);
                }
                program
                    .segment_at(
                        timing,
                        VolumePlacement::Session { origin },
                        performance,
                        frames,
                    )
                    .map_err(|_| Error::InvalidSourceSpan)?
                    .map(|ramp| VolumeSlice {
                        at: performance,
                        ramp,
                        terminal: console_endpoint.map(|e| e.frame),
                    })
                    .map(Some)
                    .ok_or(Error::InvalidSourceSpan)
            }
            RouteOwner::SessionAudio { .. } | RouteOwner::Silence => Ok(None),
        }
    };
    runtime
        .process_segment_with_volume(
            track_id,
            performance_frame,
            source_start,
            output,
            onsets,
            suppressions,
            events,
            Some(&clock),
            resources,
            &mut lookup,
            parameter_values,
        )
        .map_err(crate::mutable_instrument::MutableInstrumentRuntimeError::render_error)
}

/// One borrowed effective performance timeline shared by every voice in a chunk.
pub(super) struct InstrumentClock<'a, T: ?Sized> {
    pub(super) timing: &'a T,
    pub(super) start: u64,
}
impl<T: MidiTiming + ?Sized> hex_lattice::NativeInstrumentClock for InstrumentClock<'_, T> {
    fn tempo_micro_bpm_at_offset(&self, offset: usize) -> Option<u32> {
        self.timing
            .tempo_micro_bpm_at_frame(self.start.checked_add(offset as u64)?)
    }
    fn performance_frame_at_offset(&self, offset: usize) -> Option<u64> {
        self.start.checked_add(u64::try_from(offset).ok()?)
    }
    fn transport_is_running(&self) -> Option<bool> {
        Some(true)
    }
    fn beat_at_offset(&self, offset: usize) -> Option<u64> {
        let frame = self.start.checked_add(u64::try_from(offset).ok()?)?;
        self.timing.frame_to_beat(frame).ok().map(|beat| beat.raw())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_lattice::NativeInstrumentClock;

    #[test]
    fn instrument_clock_reports_running_transport_and_performance_offsets() {
        let project = hex_project::Project::new(48_000).unwrap();
        let timing = crate::PreparedTempoMap::prepare(project.tempo_map().clone(), 48_000).unwrap();
        let clock = InstrumentClock {
            timing: &timing,
            start: 100,
        };
        assert_eq!(clock.transport_is_running(), Some(true));
        assert_eq!(clock.performance_frame_at_offset(24), Some(124));
        assert!(clock.tempo_micro_bpm_at_offset(0).is_some());
        assert!(clock.beat_at_offset(0).is_some());
    }
}
