//! Shared zero-allocation sampler resource scoping, automation origin, and console volume-program
//! setup helpers used by both `mutable_chunk` and `shared_source`.
use super::instrument_automation;
use crate::SessionMidiOrigin;
use crate::midi_clip_volume::{
    PreparedMidiClipVolume, VolumeEndpoint, VolumePlacement, VolumeRamp,
};
use crate::mutable_instrument::VolumeSlice;
use crate::native_device_resources::{SamplerSourcesRef, ScopedDeviceResources};
use crate::prepared_tempo::MidiTempoBreaks;
use crate::session_playback::SessionPlaybackError;
use hex_project::TrackId;

/// Resolve the optional sampler resource lease for `track_id`, returning the canonical diagnostic
/// error if the track's scope is unavailable.
pub(super) fn resolve_track_sampler_resources<'a>(
    sources: Option<SamplerSourcesRef<'a>>,
    track_id: TrackId,
) -> Result<Option<ScopedDeviceResources<'a>>, SessionPlaybackError> {
    sources
        .filter(|sources| sources.has_track(track_id))
        .map(|sources| sources.resources(track_id))
        .transpose()
        .map_err(|_| {
            SessionPlaybackError::MutableInstrumentDiagnostic(
                "sampler resource scope is unavailable",
            )
        })
}

/// Borrow `scoped` as a [`hex_lattice::DeviceResources`] trait object, falling back to
/// [`hex_lattice::EMPTY_DEVICE_RESOURCES`] when absent.
pub(super) fn as_device_resources<'a>(
    scoped: &'a Option<ScopedDeviceResources<'_>>,
) -> &'a dyn hex_lattice::DeviceResources {
    scoped.as_ref().map_or(
        &hex_lattice::EMPTY_DEVICE_RESOURCES as &dyn hex_lattice::DeviceResources,
        |resources| resources,
    )
}

/// Construct a running source-automation origin for a mutable or shared-source render chunk.
pub(super) fn running_automation_origin(
    arrangement_frame: u64,
    performance_frame: u64,
    arrangement_loop_pass: u64,
) -> instrument_automation::Origin {
    instrument_automation::Origin {
        arrangement: Some(arrangement_frame),
        performance: performance_frame,
        pass: arrangement_loop_pass,
        running: true,
    }
}

/// Evaluate whether a console MIDI clip's volume program has a historical end gain or finite
/// endpoint under `timing`.
pub(super) fn console_volume_bounds<T: MidiTempoBreaks + ?Sized>(
    program: Option<(SessionMidiOrigin, &PreparedMidiClipVolume)>,
    timing: &T,
) -> Result<(bool, Option<VolumeEndpoint>), ()> {
    let historical = program
        .map(|(origin, program)| {
            program.historical_end_gain(timing, VolumePlacement::Session { origin })
        })
        .transpose()
        .map_err(|_| ())?
        .flatten()
        .is_some();
    let endpoint = if historical {
        None
    } else {
        program
            .map(|(origin, program)| {
                program.finite_endpoint(timing, VolumePlacement::Session { origin })
            })
            .transpose()
            .map_err(|_| ())?
            .flatten()
    };
    Ok((historical, endpoint))
}

/// Unity-gain volume slice anchored at sample offset `at` with no terminal endpoint.
pub(super) fn unity_volume_slice(at: u64) -> VolumeSlice {
    VolumeSlice {
        at,
        terminal: None,
        ramp: VolumeRamp {
            start_frame: 0,
            end_frame: u64::MAX,
            start_gain: 1.0,
            end_gain: 1.0,
            next_boundary: None,
        },
    }
}
