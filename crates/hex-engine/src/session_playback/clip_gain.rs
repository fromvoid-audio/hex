//! Scalar gain custody at the existing analytic release/import cut. Retired
//! voices retain no clip curve, timing map, source or extra resource owner.
use super::*;
use crate::midi_clip_volume::VolumePlacement;

pub(super) fn at_cut(
    previous: &SessionPlayback,
    index: usize,
    owner: RouteOwner,
    clip: ClipId,
    source_frame: u64,
    performance_frame: u64,
) -> Result<(f64, bool), SessionPlaybackError> {
    let invalid = |_| SessionPlaybackError::MutableInstrumentEvents;
    match owner {
        RouteOwner::Arrangement => {
            let track = previous
                .revision
                .arrangement()
                .tracks()
                .get(index)
                .ok_or(SessionPlaybackError::UnknownTrack)?;
            arrangement(
                track,
                previous.revision.session().timing(),
                clip,
                source_frame,
            )
        }
        RouteOwner::Session {
            origin,
            clip: active,
        } if active == clip => {
            let timing = timing_view_for(
                previous.revision.session(),
                previous.periodic_tempo(),
                previous.anchor,
            );
            let facade = crate::session_inventory::EffectiveSessionMidi::new(
                previous.revision.session(),
                previous.midi_admission.as_ref(),
            );
            let track = previous
                .routes
                .get(index)
                .ok_or(SessionPlaybackError::UnknownTrack)?;
            let prepared = facade
                .midi_clip_for_track(clip, track.id)
                .ok_or(SessionPlaybackError::UnknownClip)?;
            let Some(program) = prepared.volume_program() else {
                return Ok((1.0, false));
            };
            let placement = VolumePlacement::Session { origin };
            if let Some(gain) = program
                .historical_end_gain(&timing, placement)
                .map_err(invalid)?
            {
                return Ok((gain, true));
            }
            if let Some(endpoint) = program
                .finite_endpoint(&timing, placement)
                .map_err(invalid)?
                && performance_frame >= endpoint.frame
            {
                return Ok((endpoint.gain, true));
            }
            let floor = previous.anchor.map_or(0, |a| a.cut_frame);
            let at = performance_frame.saturating_sub(1).max(floor);
            let ramp = program
                .segment_at(&timing, placement, at, 1)
                .map_err(invalid)?
                .ok_or(SessionPlaybackError::MutableInstrumentEvents)?;
            Ok((ramp.gain_at(performance_frame).map_err(invalid)?, false))
        }
        RouteOwner::Session { .. } | RouteOwner::SessionAudio { .. } | RouteOwner::Silence => {
            Ok((1.0, true))
        }
    }
}

pub(super) fn arrangement<T: crate::prepared_tempo::MidiTempoBreaks + ?Sized>(
    track: &crate::Track,
    timing: &T,
    clip: ClipId,
    source_frame: u64,
) -> Result<(f64, bool), SessionPlaybackError> {
    let invalid = |_| SessionPlaybackError::MutableInstrumentEvents;
    let Some(row) = track
        .instrument
        .as_ref()
        .and_then(|i| i.volumes.as_ref())
        .and_then(|v| v.clip(clip))
    else {
        return Ok((1.0, false));
    };
    let Some(endpoint) = row.endpoint else {
        return Err(SessionPlaybackError::MutableInstrumentEvents);
    };
    if source_frame >= endpoint.frame {
        return Ok((endpoint.gain, true));
    }
    let at = source_frame.saturating_sub(1).max(row.start_frame);
    let ramp = row.segment_or_hold(timing, at, 1).map_err(invalid)?;
    Ok((ramp.gain_at(source_frame).map_err(invalid)?, false))
}
