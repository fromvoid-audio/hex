//! Compiled immutable project graph and post-sum master gain stage.
use crate::{
    Frame, GainAutomationCapacity, Invalid, MAX_OUTPUT_CAPTURES, OutputCaptureId,
    PreparedAudioClipView, ReturnBus, StreamedSourceMetadata, Track, owned_shared, prepared_tempo,
    project_output, return_feed_plan, session_playback, shared_analytic, signal_graph,
    source_effect_stage,
};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct Project {
    pub(crate) project_track_ids: project_output::ProjectTrackIds,
    pub(crate) audio_clip_views: Vec<PreparedAudioClipView>,
    pub(crate) sample_rate: u32,
    pub(crate) tracks: Vec<Track>,
    pub(crate) return_buses: Vec<ReturnBus>,
    pub(crate) return_feed_plan: return_feed_plan::ReturnFeedPlan,
    pub(crate) signal_graph: Option<Arc<signal_graph::SignalGraph>>,
    pub(crate) tempo_map: Option<hex_project::TempoMap>,
    // Validated with the immutable ordinary map; native roots borrow their
    // prepared Session inventory clock instead of retaining a second map.
    pub(crate) tempo_max_micro_bpm: u32,
    pub(crate) midi_routing: Vec<(hex_project::TrackId, hex_project::TrackId)>,
    pub(crate) loop_region: Option<hex_project::LoopRegion>,
    pub(crate) master_gain: PreparedMasterGain,
    pub(crate) main_device_chain: Arc<[hex_lattice::PreparedRuntimeInstance]>,
    pub(crate) source_effects: Vec<source_effect_stage::PreparedSourceEffects>,
    // Final causal arrival after Main, separate from the pre-Main summing graph.
    pub(crate) main_output_latency_frames: u32,
    pub(crate) audible_end_frame: Option<u64>,
    pub(crate) has_streamed_sources: bool,
    // A legacy whole-project renderer cannot silently omit recorded parameters.
    pub(crate) requires_scalar_automation: bool,
    pub(crate) shared_contexts:
        owned_shared::OwnedShared<shared_analytic::SharedAnalyticContextTable>,
}
impl Project {
    pub(crate) fn midi_volume_timing(&self) -> Option<prepared_tempo::CanonicalMidiTiming<'_>> {
        self.tempo_map.as_ref().map(|map| {
            prepared_tempo::CanonicalMidiTiming::for_validated(map, self.tempo_max_micro_bpm)
        })
    }
    pub(crate) fn new_main_device_runtime(
        &self,
    ) -> Result<hex_lattice::NativeRuntimeChain, hex_lattice_contract::DeviceContractError> {
        let mut instances = Vec::new();
        instances
            .try_reserve_exact(self.main_device_chain.len())
            .map_err(|_| hex_lattice_contract::DeviceContractError::RuntimeAllocation)?;
        instances.extend(self.main_device_chain.iter().cloned());
        hex_lattice::NativeRuntimeChain::new_prepared(instances)
    }

    /// Worker-only observation of accepted transformed PCM, independent of cache residency.
    pub fn audio_clip_views(&self) -> &[PreparedAudioClipView] {
        &self.audio_clip_views
    }

    /// Accounted Arrangement note-bank bytes, including each bank's entry
    /// allocation and fixed bank/Arc owner overhead, plus separately retained
    /// clip-volume rows/control metadata charged to the same prepared MIDI budget.
    pub fn midi_note_bank_storage_bytes(&self) -> Result<usize, Invalid> {
        self.tracks
            .iter()
            .filter_map(|track| track.instrument.as_ref())
            .try_fold(0_usize, |total, instrument| {
                total
                    .checked_add(instrument.prepared_storage_bytes()?)
                    .ok_or(Invalid("midi note bank byte count overflow"))
            })
    }

    /// Number of compact sampler indices retained by Arrangement note spans.
    pub fn midi_note_sampler_reference_count(&self) -> Result<usize, Invalid> {
        self.tracks
            .iter()
            .filter_map(|track| track.instrument.as_ref())
            .try_fold(0_usize, |total, instrument| {
                total
                    .checked_add(if instrument.mutable {
                        0
                    } else {
                        instrument.spans.len()
                    })
                    .ok_or(Invalid("midi note bank reference count overflow"))
            })
    }

    pub fn new(sample_rate: u32, tracks: Vec<Track>) -> Result<Self, Invalid> {
        Self::with_loop_region(sample_rate, tracks, None)
    }
    /// Compile an arrangement and its optional canonical playback loop region.
    pub fn with_loop_region(
        sample_rate: u32,
        tracks: Vec<Track>,
        loop_region: Option<hex_project::LoopRegion>,
    ) -> Result<Self, Invalid> {
        Self::with_master_gain(sample_rate, tracks, loop_region, 1.0)
    }
    /// Compile an arrangement with the validated linear master output gain.
    pub fn with_master_gain(
        sample_rate: u32,
        tracks: Vec<Track>,
        loop_region: Option<hex_project::LoopRegion>,
        master_gain: f32,
    ) -> Result<Self, Invalid> {
        if sample_rate == 0 {
            return Err(Invalid("sample rate must be positive"));
        }
        let master_gain = PreparedMasterGain::new(master_gain)?;
        let has_streamed_sources = tracks.iter().any(|track| {
            track.clips.iter().any(|clip| {
                clip.metadata
                    .as_deref()
                    .is_some_and(|metadata| metadata.streamed.is_some())
            })
        });
        if tracks.iter().flat_map(|t| &t.clips).any(|clip| {
            clip.audio
                .as_ref()
                .is_some_and(|audio| audio.sample_rate != sample_rate)
                || clip
                    .metadata
                    .as_deref()
                    .and_then(|metadata| metadata.streamed.as_deref())
                    .is_some_and(|source| source.project_rate() != sample_rate)
        }) {
            return Err(Invalid(
                "prepared audio sources must match the project rate",
            ));
        }
        Ok(Self {
            project_track_ids: project_output::ProjectTrackIds::prepare(&tracks),
            audio_clip_views: Vec::new(),
            source_effects: Vec::new(),
            audible_end_frame: tracks
                .iter()
                .flat_map(|track| track.clips.iter().map(|clip| clip.end))
                .max(),
            sample_rate,
            tracks,
            return_buses: Vec::new(),
            return_feed_plan: return_feed_plan::ReturnFeedPlan::default(),
            signal_graph: None,
            tempo_map: None,
            tempo_max_micro_bpm: 0,
            midi_routing: Vec::new(),
            loop_region,
            master_gain,
            main_device_chain: Arc::from([]),
            main_output_latency_frames: 0,
            has_streamed_sources,
            requires_scalar_automation: false,
            shared_contexts: owned_shared::OwnedShared::new_owned(
                shared_analytic::SharedAnalyticContextTable::default(),
            ),
        })
    }

    pub(crate) fn with_compiled_tracks(
        sample_rate: u32,
        tracks: Vec<Track>,
        return_buses: Vec<ReturnBus>,
        loop_region: Option<hex_project::LoopRegion>,
        master_gain: f32,
        shared_contexts: Arc<shared_analytic::SharedAnalyticContextTable>,
    ) -> Result<Self, Invalid> {
        let mut project = Self::with_master_gain(sample_rate, tracks, loop_region, master_gain)?;
        project.return_buses = return_buses;
        project.shared_contexts = owned_shared::OwnedShared::untracked(shared_contexts);
        project.audible_end_frame = project
            .tracks
            .iter()
            .flat_map(|track| {
                track
                    .clips
                    .iter()
                    .map(|clip| clip.end)
                    .chain(track.instrument.iter().flat_map(|instrument| {
                        instrument
                            .timeline_end
                            .into_iter()
                            .chain(instrument.spans.iter().map(|span| span.end))
                    }))
            })
            .max();
        Ok(project)
    }

    pub(crate) fn shared_contexts(&self) -> &Arc<shared_analytic::SharedAnalyticContextTable> {
        &self.shared_contexts
    }
    pub fn gain_automation_capacity(&self) -> GainAutomationCapacity {
        let mut capacity = GainAutomationCapacity::default();
        for track in &self.tracks {
            if let Some(envelope) = &track.gain_envelope {
                let actual = envelope.capacity();
                capacity.authored_points += actual.authored_points;
                capacity.prepared_knots += actual.prepared_knots;
                capacity.allocated_knot_slots += actual.allocated_knot_slots;
                capacity.allocated_knot_bytes += actual.allocated_knot_bytes;
                capacity.authored_units += actual.authored_units;
                capacity.prepared_touch_spans += actual.prepared_touch_spans;
                capacity.prepared_touch_steps += actual.prepared_touch_steps;
                capacity.allocated_touch_bytes += actual.allocated_touch_bytes;
            }
        }
        capacity
    }
    /// Known causal startup delay; compensation does not remove physical latency.
    pub fn main_latency_frames(&self) -> u32 {
        self.main_output_latency_frames
    }
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
    pub fn loop_region(&self) -> Option<hex_project::LoopRegion> {
        self.loop_region
    }
    /// Exclusive end of all compiled audio and instrument output, if any.
    pub fn audible_end_frame(&self) -> Option<u64> {
        self.audible_end_frame
    }

    /// Conservative causal end of source material at pre-Main capture taps.
    ///
    /// The graph's pre-Main arrival includes routing compensation but excludes
    /// serial Main processors. All selected taps may therefore be covered even
    /// if some finish earlier. Processor residual tails are not included.
    pub fn untrimmed_capture_source_end_frame(&self) -> Result<Option<u64>, Invalid> {
        let latency = self
            .signal_graph
            .as_ref()
            .map_or(0, |graph| graph.main_latency());
        self.audible_end_frame
            .map(|end| {
                end.checked_add(u64::from(latency))
                    .ok_or(Invalid("pre-Main capture range overflow"))
            })
            .transpose()
    }

    /// Conservative causal end for the selected pre-Main outputs, including
    /// declared residual tails along their contributing graph paths. Unknown,
    /// over-budget or overflowing tails require an explicit export range.
    pub fn untrimmed_capture_end_frame(
        &self,
        captures: &[OutputCaptureId],
    ) -> Result<Option<u64>, Invalid> {
        if captures.len() > MAX_OUTPUT_CAPTURES {
            return Err(Invalid("too many capture outputs"));
        }
        let Some(source_end) = self.untrimmed_capture_source_end_frame()? else {
            return Ok(None);
        };
        let tail = match &self.signal_graph {
            Some(graph) => graph.output_tail_frames(self, Some(captures))?,
            None => 0,
        };
        self.checked_output_tail_end(source_end, tail).map(Some)
    }

    pub(crate) fn checked_output_tail_end(
        &self,
        source_end: u64,
        tail: u64,
    ) -> Result<u64, Invalid> {
        let limit = u64::from(self.sample_rate)
            .checked_mul(session_playback::MAX_FINITE_EFFECT_TAIL_SECONDS)
            .ok_or(Invalid("automatic output drain overflow"))?;
        if tail > limit {
            return Err(Invalid(
                "automatic output drain exceeds its bounded horizon",
            ));
        }
        source_end
            .checked_add(tail)
            .ok_or(Invalid("automatic output range overflow"))
    }

    /// Causal Main output end for an untrimmed, automatically sized offline
    /// range. Ordinary source-length exports trim startup separately and must
    /// continue to use `audible_end_frame`. Explicit ranges never use this bound.
    pub fn untrimmed_main_end_frame(&self) -> Result<Option<u64>, Invalid> {
        let Some(source_end) = self.audible_end_frame else {
            return Ok(None);
        };
        let tail = match &self.signal_graph {
            Some(graph) => graph.output_tail_frames(self, None)?,
            None => {
                let mut tail = 0_u64;
                for device in self
                    .main_device_chain
                    .iter()
                    .filter(|device| !device.bypassed)
                {
                    let output = device.definition.as_ref().map_or(
                        hex_lattice::OutputTail::Finite { max_tail_frames: 0 },
                        hex_lattice::PreparedProcessorFactory::output_tail,
                    );
                    let hex_lattice::OutputTail::Finite { max_tail_frames } = output else {
                        return Err(Invalid("automatic output tail is unbounded"));
                    };
                    tail = tail
                        .checked_add(max_tail_frames)
                        .ok_or(Invalid("automatic output tail overflows"))?;
                }
                tail
            }
        };
        let causal_end = source_end
            .checked_add(u64::from(self.main_output_latency_frames))
            .ok_or(Invalid("Main export range overflow"))?;
        self.checked_output_tail_end(causal_end, tail).map(Some)
    }

    pub fn device_meter(
        &self,
        device_id: u64,
    ) -> Option<&std::sync::Arc<hex_lattice::DeviceMeter>> {
        for track in &self.tracks {
            if track
                .instrument_id
                .is_some_and(|id| id.value() == device_id)
            {
                return Some(&track.instrument_meter);
            }
            if let Some(device) = track
                .device_chain
                .iter()
                .find(|device| device.device_id == device_id)
            {
                return Some(&device.meter);
            }
        }
        self.return_buses
            .iter()
            .flat_map(|bus| bus.device_chain.iter())
            .chain(self.main_device_chain.iter())
            .find(|device| device.device_id == device_id)
            .map(|device| &device.meter)
    }

    pub fn tracks(&self) -> &[Track] {
        &self.tracks
    }

    pub(crate) fn return_buses(&self) -> &[ReturnBus] {
        &self.return_buses
    }

    pub(crate) fn streamed_sources(&self) -> impl Iterator<Item = &StreamedSourceMetadata> {
        self.tracks
            .iter()
            .flat_map(|track| track.clips.iter())
            .filter_map(|clip| clip.metadata.as_ref().and_then(|m| m.streamed.as_deref()))
    }
    /// Whether any clip needs streamed pages to produce audio.
    pub fn contains_streamed_sources(&self) -> bool {
        self.has_streamed_sources
    }

    pub fn master_mix(&self) -> PreparedMasterGain {
        self.master_gain
    }
}

/// Validated post-sum master stage, reusable by other track-source mixes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PreparedMasterGain(f32);
impl PreparedMasterGain {
    pub fn new(gain: f32) -> Result<Self, Invalid> {
        if !gain.is_finite() || !(0.0..=2.0).contains(&gain) {
            return Err(Invalid("master gain must be finite and within 0..2"));
        }
        Ok(Self(gain))
    }

    pub fn value(self) -> f32 {
        self.0
    }

    /// Apply after all track contributions have been accumulated.
    pub fn apply(self, output: &mut [Frame]) {
        if self.0 != 1.0 {
            for frame in output {
                frame[0] *= self.0;
                frame[1] *= self.0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Audio, Clip};

    #[test]
    fn automatic_tail_end_refuses_overflow_and_excessive_drain() {
        let project = Project::new(48_000, Vec::new()).unwrap();
        let limit = 48_000 * session_playback::MAX_FINITE_EFFECT_TAIL_SECONDS;
        assert_eq!(
            project.checked_output_tail_end(1, limit).unwrap(),
            1 + limit
        );
        assert!(project.checked_output_tail_end(1, limit + 1).is_err());
        assert!(project.checked_output_tail_end(u64::MAX, 1).is_err());
    }

    #[test]
    fn project_rejects_mismatched_track_sample_rate_and_invalid_master_gain() {
        let a = Audio::new(44_100, vec![[0.0; 2]]).unwrap();
        let t = Track::new(vec![Clip::new(a, 0, 1, false).unwrap()], 1.0, false).unwrap();
        assert!(Project::new(48_000, vec![t]).is_err());
        assert!(PreparedMasterGain::new(-0.1).is_err());
        assert!(PreparedMasterGain::new(2.5).is_err());
        assert!(PreparedMasterGain::new(f32::NAN).is_err());
    }
}
