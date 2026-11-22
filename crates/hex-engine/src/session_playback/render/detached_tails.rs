//! Detached tail sends and mutable tail rendering.
use super::*;

/// Contribute a detached source to each retained post-fader send before
/// the ordinary Return owner processes its complete input once.
pub(super) fn add_detached_source_sends(
    sends: &crate::mutable_instrument::DetachedSourceSends,
    returns: &[crate::ReturnBus],
    scratch: &mut [[Frame; 256]],
    mix: crate::PreparedTrackMix,
    output: &[Frame],
) -> Result<(), SessionPlaybackError> {
    for (id, gain) in sends.iter() {
        let index = returns
            .iter()
            .position(|bus| bus.id == id)
            .ok_or(SessionPlaybackError::DeviceRuntime)?;
        let destination = scratch
            .get_mut(index)
            .ok_or(SessionPlaybackError::DeviceRuntime)?;
        for (target, source) in destination.iter_mut().zip(output.iter().copied()) {
            let contribution = mix.apply_frame(source);
            target[0] += contribution[0] * gain;
            target[1] += contribution[1] * gain;
        }
    }
    Ok(())
}

/// A retained suffix is post-fader. Grouped rendering adds it to its original
/// default route and, only with a retained graph proof, its sends and feeds.
#[allow(clippy::too_many_arguments)]
pub(super) fn emit_detached_tail(
    graph: Option<&crate::signal_graph::SignalGraph>,
    owner: Option<TrackId>,
    mix: crate::PreparedTrackMix,
    tail: &mut [Frame],
    output: &mut [Frame],
    offset: usize,
    capture: &mut Option<&mut dyn CaptureSink>,
    routed: &mut Option<&mut [Frame]>,
    routed_sends: &mut Option<&mut [Frame]>,
    allow_sends: bool,
) -> Result<(), SessionPlaybackError> {
    if let Some(destinations) = routed.as_deref_mut() {
        if tail.len() != 1 {
            return Err(SessionPlaybackError::DeviceRuntime);
        }
        let owner = owner.ok_or(SessionPlaybackError::DeviceRuntime)?;
        let index = graph
            .and_then(|graph| graph.track_index(owner))
            .ok_or(SessionPlaybackError::UnknownTrack)?;
        let destination = destinations
            .get_mut(index)
            .ok_or(SessionPlaybackError::UnknownTrack)?;
        mix.add_frame(destination, tail[0]);
        if allow_sends {
            let send_destination = routed_sends
                .as_deref_mut()
                .and_then(|destinations| destinations.get_mut(index))
                .ok_or(SessionPlaybackError::UnknownTrack)?;
            mix.add_frame(send_destination, tail[0]);
        }
    } else {
        for (destination, source) in output.iter_mut().zip(tail.iter().copied()) {
            mix.add_frame(destination, source);
        }
        if let Some(capture) = capture.as_mut() {
            let owner = owner.ok_or(SessionPlaybackError::DeviceRuntime)?;
            for frame in tail.iter_mut() {
                *frame = mix.apply_frame(*frame);
            }
            capture.add(OutputCaptureId::Track(owner), offset, tail)?;
        }
    }
    Ok(())
}

pub(super) fn process_mutable_tail<T: MidiTiming + ?Sized>(
    tail: &mut super::MutableInstrumentTail,
    onsets: &mut MutableOnsetBuffer,
    events: &mut Vec<hex_lattice::NativeInstrumentEvent>,
    performance_frame: u64,
    output: &mut [Frame],
    timing: &T,
    playing: bool,
) -> Result<(), SessionPlaybackError> {
    let track = tail.track_id;
    tail.render_source(
        output,
        SessionPlaybackError::DeviceRuntime,
        |runtime, output| {
            onsets.clear();
            runtime
                .with_frozen_parameters(|runtime, values| {
                    if !playing && runtime.is_source_owner() {
                        let clock = runtime
                            .source_monitor_clock(
                                timing
                                    .frame_to_beat(performance_frame)
                                    .map_err(|_| SessionPlaybackError::TempoConversion)?
                                    .raw(),
                                timing
                                    .tempo_micro_bpm_at_frame(performance_frame)
                                    .ok_or(SessionPlaybackError::TempoConversion)?,
                                timing.sample_rate(),
                            )
                            .map_err(|error| error.render_error())?;
                        runtime
                            .process_frozen_tail_segment(
                                None,
                                performance_frame,
                                output,
                                onsets,
                                events,
                                Some(&clock),
                                values,
                            )
                            .map_err(|error| error.render_error())
                    } else {
                        runtime
                            .process_frozen_tail_segment(
                                Some(track),
                                performance_frame,
                                output,
                                onsets,
                                events,
                                Some(&InstrumentClock {
                                    timing,
                                    start: performance_frame,
                                }),
                                values,
                            )
                            .map_err(|error| error.render_error())
                    }
                })
                .map_err(|error| error.render_error())?
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_detached_source_sends_is_a_noop() {
        let sends = crate::mutable_instrument::DetachedSourceSends::default();
        let mut scratch = [[[0.0; 2]; 256]];
        add_detached_source_sends(
            &sends,
            &[],
            &mut scratch,
            crate::PreparedTrackMix::identity(),
            &[[0.5, -0.5]; 4],
        )
        .unwrap();
        assert_eq!(scratch[0][0], [0.0, 0.0]);
    }
}
