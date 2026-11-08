//! One bounded authored event stream and generator at each explicit receiver.
//! Contributor clip/track identities and volume clocks remain independent; all
//! callback scratch belongs to the existing playback and source runtime owners.
use super::*;

impl SessionPlayback {
    pub(super) fn has_analytic_release_output(&self) -> bool {
        self.releases
            .iter()
            .any(|release| release.end_frame() > self.performance_frame)
    }
    pub(super) fn advance_paused_analytic_releases(
        &mut self,
        frames: usize,
    ) -> Result<(), SessionPlaybackError> {
        let frames = u64::try_from(frames).map_err(|_| SessionPlaybackError::ArithmeticOverflow)?;
        for release in &mut self.release_scratch {
            release.monitor_offset = release
                .monitor_offset
                .checked_add(frames)
                .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
        }
        self.release_scratch
            .retain(|release| release.end_frame() > self.performance_frame);
        Ok(())
    }
}

impl SessionPlayback {
    /// Check the complete due scene against one source before changing gates.
    /// Only route-owner values are temporarily overlaid; all are restored even
    /// on refusal. No prepared/runtime/source custody is moved.
    pub(super) fn preflight_shared_due_routes(
        &mut self,
        frame: u64,
    ) -> Result<(), SessionPlaybackError> {
        if !self.revision.source_routing().has_shared_sources()
            || !self
                .route_scratch
                .iter()
                .any(|route| route.pending.is_some_and(|pending| pending.frame <= frame))
        {
            return Ok(());
        }
        let mut original = [RouteOwner::Silence; MAX_TRACKS];
        for (index, route) in self.route_scratch.iter().enumerate() {
            original[index] = route.owner;
        }
        let result = (|| {
            for index in 0..self.route_scratch.len() {
                let Some(pending) = self.route_scratch[index]
                    .pending
                    .filter(|pending| pending.frame <= frame)
                else {
                    continue;
                };
                let is_audio = matches!(pending.target, RouteTarget::Session(clip) if self.revision.session().audio_clip(clip).is_some());
                let beat = if matches!(pending.target, RouteTarget::Session(_)) && !is_audio {
                    timing_view_for(self.revision.session(), self.periodic_tempo(), self.anchor)
                        .frame_to_beat(frame)
                        .map_err(|_| SessionPlaybackError::TempoConversion)?
                } else {
                    pending.beat
                };
                self.route_scratch[index].owner =
                    self.owner_for_target(pending.target, beat, frame)?;
            }
            for index in 0..self.route_scratch.len() {
                if self.route_scratch[index]
                    .pending
                    .is_some_and(|pending| pending.frame <= frame)
                {
                    self.check_shared_transition_capacity(
                        index,
                        self.route_scratch[index].owner,
                        frame,
                        true,
                    )?;
                }
            }
            Ok(())
        })();
        for (route, owner) in self.route_scratch.iter_mut().zip(original) {
            route.owner = owner;
        }
        result
    }
}

pub(super) fn shared_source_has_route(
    revision: &PreparedRevision,
    routes: &[TrackRoute],
    receiver: usize,
) -> Result<bool, SessionPlaybackError> {
    for &contributor in revision.source_routing().contributors(receiver) {
        let index = usize::from(contributor);
        if revision.arrangement().tracks()[index].source_input_enabled()
            && !matches!(
                effective_midi_route(revision, routes, index)?.owner,
                RouteOwner::Silence
            )
        {
            return Ok(true);
        }
    }
    Ok(false)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn render_source_instrument_chunk<T: crate::prepared_tempo::MidiTempoBreaks>(
    context: RouteSourceContext<'_, T>,
    routes: &[TrackRoute],
    historical_floor: Option<BeatTime>,
    route_floors: &[Option<BeatTime>],
    spans: &mut MidiSpanBuffer,
    performance_frame: u64,
    arrangement_frame: u64,
    output: &mut [Frame],
) -> Result<(), SessionPlaybackError> {
    let routing = context.revision.source_routing();
    if !routing.is_shared(context.index) {
        return render_route_source_chunk::<dyn MediaPages, _>(
            context,
            spans,
            performance_frame,
            arrangement_frame,
            output,
            None,
            SourceStage::Instrument,
        );
    }
    if !context.revision.arrangement().tracks()[context.index].receives_source_audio() {
        return Ok(());
    }
    for &contributor in routing.contributors(context.index) {
        let index = usize::from(contributor);
        if !context.revision.arrangement().tracks()[index].source_input_enabled() {
            continue;
        }
        render_route_source_chunk::<dyn MediaPages, _>(
            RouteSourceContext {
                revision: context.revision,
                midi: context.midi,
                route: effective_midi_route(context.revision, routes, index)?,
                historical_floor: historical_floor
                    .into_iter()
                    .chain(route_floors.get(index).copied().flatten())
                    .max(),
                suppressions: context.suppressions,
                continuations: context.continuations,
                index,
                raw: true,
                loop_fade: None,
                timing: context.timing,
                table: context.table,
            },
            spans,
            performance_frame,
            arrangement_frame,
            output,
            None,
            SourceStage::Instrument,
        )?;
    }
    Ok(())
}

pub(super) fn render_mutable_source_chunk<T: crate::prepared_tempo::MidiTempoBreaks + ?Sized>(
    context: MutableInstrumentChunkContext<'_, T>,
    receiver: usize,
    output: &mut [Frame],
) -> Result<(), SessionPlaybackError> {
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
        chase_at_start,
        loop_region,
        timing,
    } = context;
    if !revision.arrangement().tracks()[receiver].receives_source_audio()
        || runtimes.get(receiver).and_then(Option::as_ref).is_none()
    {
        return Ok(());
    }
    let width =
        u64::try_from(output.len()).map_err(|_| SessionPlaybackError::ArithmeticOverflow)?;
    let performance_end = performance_frame
        .checked_add(width)
        .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
    let arrangement_end = arrangement_frame
        .checked_add(width)
        .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
    onsets.clear();
    let mut has_program = false;
    for &contributor in revision.source_routing().contributors(receiver) {
        let index = usize::from(contributor);
        let track = &revision.arrangement().tracks()[index];
        if !track.source_input_enabled() {
            continue;
        }
        let track_id = track
            .canonical_track_id()
            .ok_or(SessionPlaybackError::RevisionTrackMismatch)?;
        match effective_midi_route(revision, routes, index)?.owner {
            RouteOwner::Arrangement => {
                has_program |= track
                    .instrument
                    .as_ref()
                    .is_some_and(|instrument| instrument.volumes.is_some());
                let mut failed = false;
                track
                    .for_each_mutable_midi_span(arrangement_frame, arrangement_end, |span| {
                        let chase = arrangement_loop_pass > 0
                            && loop_region.is_some_and(|region| {
                                arrangement_frame == region.start()
                                    && span.start < region.start()
                                    && span.gate_end > region.start()
                            });
                        let start_chase = chase_at_start
                            && span.start < arrangement_frame
                            && span.gate_end > arrangement_frame;
                        let start = if chase || start_chase {
                            arrangement_frame
                        } else {
                            span.start
                        };
                        if start < arrangement_frame {
                            return;
                        }
                        let gate = loop_region
                            .map_or(span.gate_end, |region| span.gate_end.min(region.end()));
                        let normalized = start
                            .checked_sub(arrangement_frame)
                            .and_then(|offset| performance_frame.checked_add(offset))
                            .zip(
                                gate.checked_sub(arrangement_frame)
                                    .and_then(|offset| performance_frame.checked_add(offset)),
                            );
                        let Some((source_start, source_gate_end)) = normalized else {
                            failed = true;
                            return;
                        };
                        failed |= onsets
                            .push(crate::mutable_instrument::MutableNoteOnset {
                                key: crate::mutable_instrument::MutableOccurrenceKey::new(
                                    track_id,
                                    span.clip,
                                    span.note,
                                    span.iteration,
                                    arrangement_loop_pass,
                                ),
                                onset_beat: span.onset_beat,
                                gate_end_beat: span.gate_end_beat,
                                source_start,
                                source_gate_end,
                                pitch: span.pitch,
                                channel: span.channel,
                                velocity: span.velocity,
                            })
                            .is_err();
                    })
                    .map_err(|_| SessionPlaybackError::MutableInstrumentEvents)?;
                if failed {
                    return Err(SessionPlaybackError::MutableInstrumentEvents);
                }
            }
            RouteOwner::Session { clip, origin } => {
                let prepared = midi
                    .midi_clip_for_track(clip, track_id)
                    .ok_or(SessionPlaybackError::UnknownClip)?;
                if prepared.track != routes[index].id {
                    return Err(SessionPlaybackError::ClipTrackMismatch);
                }
                if prepared.instrument_enabled() {
                    has_program |= prepared.volume_program().is_some();
                    prepared
                        .instrument_template()
                        .query_with_origin(
                            origin,
                            timing,
                            performance_frame,
                            performance_end,
                            spans,
                        )
                        .map_err(SessionPlaybackError::Midi)?;
                    for span in spans.spans() {
                        onsets
                            .push(crate::mutable_instrument::MutableNoteOnset {
                                key: crate::mutable_instrument::MutableOccurrenceKey::new(
                                    track_id,
                                    span.clip,
                                    span.note,
                                    span.iteration,
                                    routes[index].current_incarnation,
                                ),
                                onset_beat: span.onset_beat,
                                gate_end_beat: span.gate_end_beat,
                                source_start: span.start_frame,
                                source_gate_end: span.gate_end_frame,
                                pitch: span.pitch,
                                channel: span.channel,
                                velocity: span.velocity,
                            })
                            .map_err(|_| SessionPlaybackError::MutableInstrumentEvents)?;
                    }
                }
            }
            RouteOwner::SessionAudio { .. } | RouteOwner::Silence => {}
        }
    }
    // Live input joins this same fixed event buffer through the source owner's
    // admission path; no second generator or DSP pair is introduced here.
    let runtime = runtimes
        .get_mut(receiver)
        .and_then(Option::as_mut)
        .ok_or(SessionPlaybackError::MutableInstrumentRuntime)?;
    let receiver_id = routes[receiver].id;
    let scoped = sources
        .filter(|sources| sources.has_track(receiver_id))
        .map(|sources| sources.resources(receiver_id))
        .transpose()
        .map_err(|_| {
            SessionPlaybackError::MutableInstrumentDiagnostic(
                "sampler resource scope is unavailable",
            )
        })?;
    let resources: &dyn hex_lattice::DeviceResources = scoped.as_ref().map_or(
        &hex_lattice::EMPTY_DEVICE_RESOURCES as &dyn hex_lattice::DeviceResources,
        |resources| resources,
    );
    let clock = InstrumentClock {
        timing,
        start: performance_frame,
    };
    if !has_program && !runtime.has_volume_gain() {
        return runtime
            .process_source_segment(
                performance_frame,
                output,
                onsets,
                suppressions,
                events,
                Some(&clock),
                resources,
                None,
            )
            .map_err(crate::mutable_instrument::MutableInstrumentRuntimeError::render_error);
    }
    let mut lookup = |signature: crate::mutable_instrument::MutableOccurrenceSignature,
                      performance: u64,
                      frames: usize| {
        use crate::midi_clip_volume::{VolumePlacement, VolumeRamp};
        use crate::mutable_instrument::{MutableInstrumentRuntimeError as Error, VolumeSlice};
        let key = signature.key();
        let Some(index) = revision
            .source_routing()
            .contributors(receiver)
            .iter()
            .copied()
            .map(usize::from)
            .find(|&index| routes[index].id == key.track)
        else {
            return Ok(None);
        };
        let track = &revision.arrangement().tracks()[index];
        if !track.source_input_enabled() {
            return Ok(None);
        }
        let track_id = routes[index].id;
        let midi_route =
            effective_midi_route(revision, routes, index).map_err(|_| Error::InvalidSourceSpan)?;
        let arrangement_volumes = track
            .instrument
            .as_ref()
            .and_then(|instrument| instrument.volumes.as_ref());
        let console = if let RouteOwner::Session { clip, origin } = midi_route.owner {
            midi.midi_clip_for_track(clip, track_id)
                .map(|prepared| (clip, origin, prepared))
        } else {
            None
        };
        let console_program = console.as_ref().and_then(|(_, origin, prepared)| {
            prepared.volume_program().map(|program| (*origin, program))
        });
        let console_historical = console_program
            .map(|(origin, program)| {
                program.historical_end_gain(timing, VolumePlacement::Session { origin })
            })
            .transpose()
            .map_err(|_| Error::InvalidSourceSpan)?
            .flatten()
            .is_some();
        let console_endpoint = if console_historical {
            None
        } else {
            console_program
                .map(|(origin, program)| {
                    program.finite_endpoint(timing, VolumePlacement::Session { origin })
                })
                .transpose()
                .map_err(|_| Error::InvalidSourceSpan)?
                .flatten()
        };
        let unity = |at| VolumeSlice {
            at,
            terminal: None,
            ramp: VolumeRamp {
                start_frame: 0,
                end_frame: u64::MAX,
                start_gain: 1.0,
                end_gain: 1.0,
                next_boundary: None,
            },
        };
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
            RouteOwner::Session { clip, origin } => {
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
        .process_source_segment_with_volume(
            performance_frame,
            output,
            onsets,
            suppressions,
            events,
            Some(&clock),
            resources,
            &mut lookup,
        )
        .map_err(crate::mutable_instrument::MutableInstrumentRuntimeError::render_error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_device_resources::{
        PreparedDeviceResources, ResidentDeviceAssets, ResourcePreparationError,
    };
    use hex_lattice_contract::{DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot};
    use hex_project::{
        Command, Created, Editor, MidiClipGain, MidiClipVolume, MidiClipVolumePoint,
    };
    use std::{collections::BTreeMap, sync::atomic::AtomicBool};

    /// Prepared release of the fixture's zero-authored envelope at 8 kHz.
    /// Oracles derive from it so a host declick floor only lengthens the
    /// release they expect.
    fn release_frames_of_fixture() -> usize {
        hex_devices::sampler::PreparedSamplerPlan::new(
            hex_devices::sampler::SampleShape::new(
                4,
                8000,
                hex_devices::sampler::SampleChannels::Stereo,
            )
            .unwrap(),
            8000,
            hex_devices::sampler::state::SamplerState::for_source_frames(4)
                .unwrap()
                .settings(),
        )
        .unwrap()
        .prepare_voice_coefficients(60, 127)
        .unwrap()
        .unwrap()
        .release_frames() as usize
    }

    /// Linear release level `steps` frames after note-off.
    fn release_level(steps: u64) -> f32 {
        let release = release_frames_of_fixture() as u64;
        if steps >= release {
            0.0
        } else {
            1.0 - steps as f32 / release as f32
        }
    }

    fn shared_authored_sampler_case(mode: usize) {
        std::thread::Builder::new().stack_size(8 * 1024 * 1024).spawn(move || {
            for delayed in [false, true] {
                let (revision, resources, expected, scene, _) = fixture(delayed, mode == 2, false);
                for partition in [1, 7] {
                    let mut playback = SessionPlayback::new_with_device_resources(Arc::clone(&revision), &resources).unwrap();
                    if mode == 1 { playback.apply(SessionPlaybackCommand::LaunchScene { scene, quantization: LaunchQuantization::Immediate }).unwrap(); }
                    if mode == 3 { playback.apply(SessionPlaybackCommand::StopTrack { track: revision.arrangement().tracks()[0].canonical_track_id().unwrap(), quantization: LaunchQuantization::Immediate }).unwrap(); }
                    playback.apply(SessionPlaybackCommand::Play).unwrap();
                    let mut pcm = vec![[0.0; 2]; 4008];
                    for chunk in pcm.chunks_mut(partition) { playback.render(chunk).unwrap(); }
                    let factor = if mode >= 2 { expected.1 } else { expected.0 };
                    let latency = if delayed { 3 } else { 0 };
                    let original = [[0.125, -0.25], [0.5, 0.0], [-0.125, 0.25], [0.0; 2]];
                    for (index, frame) in pcm.iter().enumerate() {
                        let sample = index.checked_sub(latency).and_then(|index| original.get(index)).copied().unwrap_or([0.0; 2]);
                        let local = if mode == 1 { [0.0; 2] } else { index.checked_sub(latency + 4000).and_then(|index| original.get(index)).copied().unwrap_or([0.0; 2]) };
                        for channel in 0..2 { assert!((frame[channel] - (sample[channel] * factor + local[channel] * expected.2)).abs() < 0.000_002, "mode={mode} delayed={delayed} partition={partition} index={index} frame={frame:?} factor={factor}"); }
                    }
                }
            }
        }).unwrap().join().unwrap();
    }

    #[test]
    fn shared_authored_sampler_arrangement() {
        shared_authored_sampler_case(0);
    }
    #[test]
    fn shared_authored_sampler_session() {
        shared_authored_sampler_case(1);
    }
    #[test]
    fn shared_authored_sampler_muted_receiver() {
        shared_authored_sampler_case(2);
    }
    #[test]
    fn shared_authored_sampler_stopped_receiver() {
        shared_authored_sampler_case(3);
    }

    #[test]
    fn shared_authored_same_pitch_stop_releases_only_its_contributor() {
        std::thread::Builder::new().stack_size(8 * 1024 * 1024).spawn(|| {
            for quantization in [LaunchQuantization::Immediate, LaunchQuantization::ThirtySecond] {
                let (revision, resources, expected, _, _) = fixture(false, false, true);
                let contributor = revision.arrangement().tracks()[1].canonical_track_id().unwrap();
                let mut playback = SessionPlayback::new_with_device_resources(revision, &resources).unwrap();
                playback.apply(SessionPlaybackCommand::Play).unwrap();
                let mut prefix = [[0.0; 2]; 64];
                playback.render(&mut prefix).unwrap();
                playback.apply(SessionPlaybackCommand::StopTrack { track: contributor, quantization }).unwrap();
                let cutoff = playback.route_status(1).unwrap().pending.map_or(64, |pending| pending.performance_frame);
                let mut output = vec![[0.0; 2]; 600];
                for chunk in output.chunks_mut(7) { playback.render(chunk).unwrap(); }
                let original = [[0.125, -0.25], [0.5, 0.0], [-0.125, 0.25], [0.0; 2]];
                for (offset, frame) in output.iter().enumerate() {
                    let frame_number = 64 + offset as u64;
                    let factor = match frame_number.checked_sub(cutoff) {
                        None => expected.0,
                        Some(steps) => expected.0 - expected.1 * (1.0 - release_level(steps)),
                    };
                    for channel in 0..2 {
                        assert!((frame[channel] - original[frame_number as usize % 4][channel] * factor).abs() < 0.000_002, "quantization={quantization:?} frame={frame_number} cutoff={cutoff} pcm={frame:?}");
                    }
                }
            }
        }).unwrap().join().unwrap();
    }

    #[test]
    fn shared_live_pause_releases_clips_and_preserves_one_source_input() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                for partition in [1, 7] {
                    let (revision, resources, expected, _, canonical) = fixture(false, false, true);
                    let contributor = canonical.tracks()[1].id();
                    let input =
                        Arc::new(crate::computer_keyboard::ComputerKeyboardInput::default());
                    input.set_key(0, Some((60, 127, 1))).unwrap();
                    let mut playback =
                        SessionPlayback::new_with_device_resources(revision, &resources).unwrap();
                    let keyboard =
                        crate::computer_keyboard::ComputerKeyboardRuntime::prepare_source_input(
                            &canonical,
                            contributor,
                            Arc::clone(&input),
                            playback.source_runtime_on_control(0),
                            None,
                            62,
                        )
                        .unwrap();
                    assert!(keyboard.is_source_input());
                    playback.computer_keyboard = Some(Box::new(keyboard));
                    assert_eq!(playback.mutable_instruments.iter().flatten().count(), 1);
                    playback.apply(SessionPlaybackCommand::Play).unwrap();
                    let mut before = [[0.0; 2]; 8];
                    for chunk in before.chunks_mut(partition) {
                        playback.render(chunk).unwrap();
                    }
                    let original = [[0.125, -0.25], [0.5, 0.0], [-0.125, 0.25], [0.0; 2]];
                    let live_gain = expected.0 / 2.5;
                    for (index, frame) in before.iter().enumerate() {
                        for channel in 0..2 {
                            assert!(
                                (frame[channel]
                                    - original[index % 4][channel] * (expected.0 + live_gain))
                                    .abs()
                                    < 0.000_002,
                                "partition={partition} index={index} pcm={frame:?}"
                            );
                        }
                    }
                    playback.apply(SessionPlaybackCommand::Pause).unwrap();
                    let mut paused = [[0.0; 2]; 8];
                    for chunk in paused.chunks_mut(partition) {
                        playback.render(chunk).unwrap();
                    }
                    for (index, frame) in paused.iter().enumerate() {
                        // Pause releases the clip note while the key holds.
                        let gain = live_gain + expected.0 * release_level(index as u64);
                        for channel in 0..2 {
                            assert!(
                                (frame[channel] - original[index % 4][channel] * gain).abs()
                                    < 0.000_002,
                                "paused partition={partition} index={index} pcm={frame:?}"
                            );
                        }
                    }
                    assert_eq!(playback.performance_frame, 8);
                    assert_eq!(playback.arrangement_frame, 8);
                    input.set_key(0, None).unwrap();
                    // Silence follows the released key's prepared release.
                    let mut silence = vec![[0.0; 2]; 32 + release_frames_of_fixture()];
                    for chunk in silence.chunks_mut(partition) {
                        playback.render(chunk).unwrap();
                    }
                    assert!(
                        silence[release_frames_of_fixture()..]
                            .iter()
                            .flatten()
                            .all(|sample| sample.abs() < 0.000_002)
                    );
                    playback.apply(SessionPlaybackCommand::Play).unwrap();
                    playback.render(&mut silence).unwrap();
                    assert!(
                        silence
                            .iter()
                            .flatten()
                            .all(|sample| sample.abs() < 0.000_002),
                        "pre-pause clips must not retrigger"
                    );
                }
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn shared_live_overflow_releases_hardware_without_cutting_clips_or_physical_keys() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                for partition in [1, 7, 256] {
                    let (revision, resources, expected, _, canonical) = fixture(false, false, true);
                    let contributor = canonical.tracks()[1].id();
                    let input = Arc::new(
                        crate::computer_keyboard::ComputerKeyboardInput::default()
                            .with_live_notes()
                            .unwrap(),
                    );
                    input.set_key(0, Some((60, 127, 1))).unwrap();
                    let mut playback =
                        SessionPlayback::new_with_device_resources(revision, &resources).unwrap();
                    let keyboard =
                        crate::computer_keyboard::ComputerKeyboardRuntime::prepare_source_input(
                            &canonical,
                            contributor,
                            Arc::clone(&input),
                            playback.source_runtime_on_control(0),
                            None,
                            62,
                        )
                        .unwrap();
                    playback.computer_keyboard = Some(Box::new(keyboard));
                    let live = input.live_notes().unwrap();
                    let activation = live.current_activation_on_control().unwrap();
                    let on = crate::computer_keyboard::LiveNoteEvent::NoteOn {
                        channel: 3,
                        pitch: 60,
                        velocity: 127,
                    };
                    live.publish(activation, on).unwrap();
                    playback.apply(SessionPlaybackCommand::Play).unwrap();
                    playback.render(&mut [[0.0; 2]; 8]).unwrap();
                    for _ in 0..256 {
                        live.publish(activation, on).unwrap();
                    }
                    assert!(
                        live.publish(
                            activation,
                            crate::computer_keyboard::LiveNoteEvent::AllNotesOff
                        )
                        .is_err()
                    );
                    let mut output = [[0.0; 2]; 512];
                    for chunk in output.chunks_mut(partition) {
                        playback.render(chunk).unwrap();
                    }
                    let original = [[0.125, -0.25], [0.5, 0.0], [-0.125, 0.25], [0.0; 2]];
                    let factor = expected.0 + expected.0 / 2.5;
                    for (index, frame) in output.iter().enumerate() {
                        // Overflow releases the hardware note at this frame.
                        let factor = factor + expected.0 / 2.5 * release_level(index as u64);
                        for channel in 0..2 {
                            assert!(
                                (frame[channel] - original[index % 4][channel] * factor).abs()
                                    < 0.000_002,
                                "partition={partition} index={index} pcm={frame:?}"
                            );
                        }
                    }
                    assert_eq!(
                        playback.mutable_instruments[0]
                            .as_ref()
                            .unwrap()
                            .proofs()
                            .filter(|proof| !proof.note_off_sent)
                            .count(),
                        2
                    );
                }
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn shared_authored_analytic_notes_sum_at_receiver_with_one_receiver_fader() {
        let mut editor = Editor::new(hex_project::Project::new(8000).unwrap(), 32).unwrap();
        let Created::Track(receiver) = editor
            .execute(Command::AddMidiTrack {
                name: "receiver".into(),
            })
            .unwrap()
        else {
            panic!("receiver")
        };
        editor
            .execute(Command::InsertInstrument {
                track: receiver,
                state: hex_project::default_instrument_state().unwrap(),
            })
            .unwrap();
        let source = editor
            .project()
            .track(receiver)
            .unwrap()
            .source_id()
            .unwrap();
        let Created::Track(contributor) = editor
            .execute(Command::AddMidiTrack {
                name: "contributor".into(),
            })
            .unwrap()
        else {
            panic!("contributor")
        };
        editor
            .execute(Command::BindTrackSource {
                track: contributor,
                source: Some(source),
            })
            .unwrap();
        let mut receiver_clip = None;
        for (track, pitch) in [(receiver, 60), (contributor, 67)] {
            let Created::Clip(clip) = editor
                .execute(Command::PlaceMidiClip {
                    track,
                    start: BeatTime::ZERO,
                    length: BeatTime::ONE,
                })
                .unwrap()
            else {
                panic!("clip")
            };
            editor
                .execute(Command::AddMidiNote {
                    clip,
                    start: BeatTime::ZERO,
                    duration: BeatTime::ONE,
                    pitch,
                    velocity: 100,
                    channel: 0,
                })
                .unwrap();
            if track == receiver {
                receiver_clip = Some(clip);
            }
        }
        editor
            .execute(Command::Gain {
                track: receiver,
                gain: 0.25,
            })
            .unwrap();
        editor
            .execute(Command::Gain {
                track: contributor,
                gain: 0.75,
            })
            .unwrap();
        let shared = editor.project().clone();
        editor.execute(Command::RemoveTrack(contributor)).unwrap();
        editor
            .execute(Command::AddMidiNote {
                clip: receiver_clip.unwrap(),
                start: BeatTime::ZERO,
                duration: BeatTime::ONE,
                pitch: 67,
                velocity: 100,
                channel: 0,
            })
            .unwrap();
        let prepare = |project| {
            Arc::new(
                PreparedRevision::prepare(
                    project,
                    &BTreeMap::new(),
                    Arc::new(crate::SineTable::new()),
                    None,
                )
                .unwrap(),
            )
        };
        let render = |revision, partition| {
            let mut playback = SessionPlayback::new(revision).unwrap();
            playback.apply(SessionPlaybackCommand::Play).unwrap();
            let mut output = [[0.0; 2]; 64];
            for chunk in output.chunks_mut(partition) {
                playback.render(chunk).unwrap();
            }
            output
        };
        let expected = render(prepare(editor.project()), 64);
        assert!(expected.iter().any(|frame| frame[0].abs() > 0.001));
        for partition in [1, 7, 64] {
            assert_eq!(render(prepare(&shared), partition), expected);
        }
    }

    fn fixture(
        delayed: bool,
        solo: bool,
        sustained: bool,
    ) -> (
        Arc<PreparedRevision>,
        PreparedDeviceResources,
        (f32, f32, f32),
        hex_project::SceneId,
        hex_project::Project,
    ) {
        let cancel = AtomicBool::new(false);
        let mut editor = Editor::new(hex_project::Project::new(8000).unwrap(), 64).unwrap();
        let Created::Asset(asset) = editor
            .execute(Command::ImportDeviceAsset {
                path: "shared.wav".into(),
                sample_rate: 8000,
                frames: 4,
            })
            .unwrap()
        else {
            panic!("asset")
        };
        let mut state = hex_devices::sampler::state::SamplerState::for_source_frames(4).unwrap();
        state.mode = if sustained {
            hex_devices::sampler::state::AuthoredMode::Classic {
                loop_enabled: true,
                loop_region: state.region,
            }
        } else {
            hex_devices::sampler::state::AuthoredMode::OneShot {
                gate: hex_devices::sampler::GateMode::Trigger,
            }
        };
        state.envelope = hex_devices::sampler::Envelope {
            attack_seconds: 0.0,
            decay_seconds: 0.0,
            sustain: 1.0,
            release_seconds: 0.0,
        };
        let state = state
            .to_native()
            .unwrap()
            .with_asset_references(vec![DeviceAssetReference {
                slot: DeviceAssetSlot::new(1).unwrap(),
                asset: DeviceAssetKey::new(asset.value()).unwrap(),
            }])
            .unwrap()
            .with_output_gain_millidb(-6000)
            .unwrap();
        let source_gain = state.output_gain_linear();
        let Created::Track(receiver) = editor
            .execute(Command::AddMidiTrack {
                name: "receiver".into(),
            })
            .unwrap()
        else {
            panic!("receiver")
        };
        editor
            .execute(Command::InsertInstrument {
                track: receiver,
                state,
            })
            .unwrap();
        let source = editor
            .project()
            .track(receiver)
            .unwrap()
            .source_id()
            .unwrap();
        let Created::Track(contributor) = editor
            .execute(Command::AddMidiTrack {
                name: "contributor".into(),
            })
            .unwrap()
        else {
            panic!("contributor")
        };
        editor
            .execute(Command::BindTrackSource {
                track: contributor,
                source: Some(source),
            })
            .unwrap();
        let Created::Scene(scene) = editor
            .execute(Command::AddScene {
                name: "scene".into(),
            })
            .unwrap()
        else {
            panic!("scene")
        };
        for (track, gain) in [(receiver, 0.5), (contributor, 2.0)] {
            let Created::Clip(arrangement) = editor
                .execute(Command::PlaceMidiClip {
                    track,
                    start: BeatTime::ZERO,
                    length: BeatTime::ONE,
                })
                .unwrap()
            else {
                panic!("arrangement")
            };
            let Created::SessionClip(console) = editor
                .execute(Command::CreateSessionMidiClip {
                    track,
                    scene,
                    name: "console".into(),
                    source_length: BeatTime::ONE,
                })
                .unwrap()
            else {
                panic!("console")
            };
            for clip in [arrangement, console] {
                editor
                    .execute(Command::AddMidiNote {
                        clip,
                        start: BeatTime::ZERO,
                        duration: BeatTime::ONE,
                        pitch: 60,
                        velocity: 127,
                        channel: 0,
                    })
                    .unwrap();
                editor
                    .execute(Command::ReplaceMidiClipVolume {
                        clip,
                        volume: MidiClipVolume::new(
                            vec![
                                MidiClipVolumePoint::linear(
                                    BeatTime::ZERO,
                                    MidiClipGain::new(gain).unwrap(),
                                ),
                                MidiClipVolumePoint::linear(
                                    BeatTime::ONE,
                                    MidiClipGain::new(gain).unwrap(),
                                ),
                            ],
                            BeatTime::ONE,
                        )
                        .unwrap(),
                    })
                    .unwrap();
            }
        }
        editor
            .execute(Command::PlaceClip {
                track: contributor,
                asset,
                start: 4000,
                duration: 4,
                source_offset: 0,
                source_length: 4,
                looping: false,
            })
            .unwrap();
        editor
            .execute(Command::Gain {
                track: receiver,
                gain: 0.25,
            })
            .unwrap();
        let builtin = hex_devices::native_registry().unwrap();
        let utility = builtin
            .default_state(hex_devices::utility::UTILITY_TYPE_ID)
            .unwrap()
            .with_output_gain_millidb(-12000)
            .unwrap();
        let local_gain = utility.output_gain_linear();
        editor
            .execute(Command::InsertDevice {
                track: contributor,
                before: None,
                state: utility,
            })
            .unwrap();
        if delayed {
            editor
                .execute(Command::InsertDevice {
                    track: receiver,
                    before: None,
                    state: crate::signal_graph::tests::state(3).unwrap(),
                })
                .unwrap();
        }
        if solo {
            editor
                .execute(Command::ToggleSolo {
                    track: contributor,
                    exclusive: false,
                })
                .unwrap();
        }
        let original = vec![[0.125, -0.25], [0.5, 0.0], [-0.125, 0.25], [0.0; 2]];
        let mut store = ResidentDeviceAssets::default();
        store
            .insert_on_control(
                (
                    asset,
                    crate::OwnedAudioSource::try_resident(8000, original.clone())
                        .ok()
                        .unwrap(),
                ),
                0,
                crate::AudioStorageLedgerReservation {
                    data_rows: 256,
                    handle_rows: 64,
                    root_rows: 32,
                    max_retained_bytes: 20_000_000,
                    max_construction_peak_bytes: 20_000_000,
                },
                4,
                &cancel,
                || Ok::<_, ResourcePreparationError>(()),
            )
            .unwrap();
        let resources = PreparedDeviceResources::prepare_on_control(
            &store,
            editor.project(),
            20_000_000,
            &cancel,
        )
        .unwrap();
        let media = BTreeMap::from([(asset, crate::Audio::new(8000, original).unwrap())]);
        let definition = crate::signal_graph::tests::Definition;
        let definitions: [&dyn hex_lattice::NativeDeviceDefinition; 2] = [
            &definition,
            builtin
                .definition(hex_devices::utility::UTILITY_TYPE_ID)
                .unwrap(),
        ];
        let effects = hex_lattice::NativeDeviceRegistry::new(&definitions).unwrap();
        let revision = PreparedRevision::prepare_with_device_resources_and_effect_registry(
            editor.project(),
            &media,
            Arc::new(crate::SineTable::new()),
            None,
            &crate::audio_transform::AudioTransformPreparation::default(),
            &resources,
            resources
                .compiler_scan_work_on_control(editor.project())
                .unwrap(),
            &cancel,
            &effects,
            None,
        )
        .unwrap();
        let source_factor = 2.5 * source_gain * 0.25;
        (
            Arc::new(revision),
            resources,
            (source_factor, 2.0 * source_gain * 0.25, local_gain),
            scene,
            editor.project().clone(),
        )
    }
}
