//! Compiled tracks, solo/mix projection, return buses, and instrument tracks.
use crate::{
    Clip, Frame, GainEnvelopeKnot, Invalid, MediaPages, clip_index, gain_envelope, loop_seam,
    midi_activity, midi_clip_volume, midi_compile, midi_note_bank, midi_template, prepared_tempo,
    render_streamed_page_span_captured, shared_analytic,
};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct Track {
    pub(crate) track_id: Option<hex_project::TrackId>,
    pub(crate) source_id: Option<hex_project::SourceId>,
    pub(crate) source_receiver_index: Option<usize>,
    pub(crate) source_audio_receiver: bool,
    pub(crate) source_input_enabled: bool,
    pub(crate) audio_content_enabled: bool,
    pub(crate) clips: Vec<Clip>,
    /// Control-built `clip_index` over `clips`; rendering never scans them.
    pub(crate) clip_index: Vec<clip_index::ClipIndexEntry>,
    /// Whether any clip pages from a stream; fixed with `clips`.
    pub(crate) has_streamed_clips: bool,
    pub(crate) mix: PreparedTrackMix,
    pub(crate) source_mix: PreparedTrackMix,
    pub(crate) gain_envelope: Option<gain_envelope::PreparedGainEnvelope>,
    pub(crate) instrument: Option<InstrumentTrack>,
    pub(crate) instrument_position: usize,
    pub(crate) ordered_prefix: Option<hex_lattice::StereoMatrix>,
    pub(crate) ordered_suffix: Option<hex_lattice::StereoMatrix>,
    pub(crate) device_chain: std::sync::Arc<[hex_lattice::PreparedRuntimeInstance]>,
    pub(crate) sends: Vec<CompiledSend>,
    pub(crate) instrument_id: Option<hex_project::DeviceId>,
    pub(crate) instrument_state: Option<hex_lattice_contract::NativeDeviceState>,
    pub(crate) foreign_instrument: Option<hex_lattice::PreparedRuntimeInstance>,
    pub(crate) tuning_identity: shared_analytic::TuningIdentity,
    pub(crate) instrument_bypassed: bool,
    pub(crate) instrument_input_gain: f32,
    pub(crate) instrument_output_gain: f32,
    pub(crate) instrument_latency: Option<u32>,
    pub(crate) instrument_runtime_latency: Option<u32>,
    pub(crate) instrument_meter: std::sync::Arc<hex_lattice::DeviceMeter>,
    pub(crate) mutable_instrument: bool,
    pub(crate) mutable_looping_source: bool,
    pub(crate) midi_source_index: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CompiledSend {
    pub return_index: usize,
    pub gain: f32,
}

#[derive(Clone, Debug)]
pub(crate) struct ReturnBus {
    pub id: hex_project::ReturnId,
    pub gain: f32,
    pub muted: bool,
    pub matrix: Option<hex_lattice::StereoMatrix>,
    pub device_chain: std::sync::Arc<[hex_lattice::PreparedRuntimeInstance]>,
    pub output_meter: std::sync::Arc<hex_lattice::DeviceMeter>,
}

/// Bounded solo projection for one immutable canonical track inventory. Group
/// ancestors remain audible, and a soloed group includes its descendants.
/// Shared-source receiver ancestors are admitted without enabling receiver-local
/// content. No authored mute or solo flags are changed.
pub(crate) struct TrackSoloSelection {
    pub(crate) any: bool,
    pub(crate) selected: [bool; hex_project::MAX_TRACKS],
}
impl TrackSoloSelection {
    pub(crate) fn prepare(project: &hex_project::Project) -> Self {
        let tracks = project.tracks();
        let any = tracks.iter().any(|track| track.soloed());
        let mut selected = [false; hex_project::MAX_TRACKS];
        if !any {
            return Self { any, selected };
        }
        // Canonical tracks are bounded and their parent IDs have already been
        // validated. Resolve indices once before walking at most the inventory.
        let mut parents = [None; hex_project::MAX_TRACKS];
        for (index, track) in tracks.iter().enumerate() {
            parents[index] = track
                .parent_group()
                .and_then(|id| tracks.iter().position(|candidate| candidate.id() == id));
        }
        for (index, track) in tracks.iter().enumerate() {
            let mut cursor = Some(index);
            for _ in 0..tracks.len() {
                let Some(current) = cursor else {
                    break;
                };
                if tracks[current].soloed() {
                    selected[index] = true;
                }
                if track.soloed() {
                    selected[current] = true;
                }
                cursor = parents[current];
            }
        }
        // Source contributors are ordinary tracks. Their own selected flags
        // remain unchanged when admitting the receiver's group path.
        for (index, track) in tracks.iter().enumerate() {
            if !selected[index] {
                continue;
            }
            let Some(source) = track.source_id() else {
                continue;
            };
            let Some(receiver) = project
                .native_source(source)
                .and_then(|source| source.receiver())
            else {
                continue;
            };
            let Some(receiver_index) = tracks
                .iter()
                .position(|candidate| candidate.id() == receiver)
            else {
                continue;
            };
            let mut cursor = parents[receiver_index];
            for _ in 0..tracks.len() {
                let Some(current) = cursor else {
                    break;
                };
                selected[current] = true;
                cursor = parents[current];
            }
        }
        Self { any, selected }
    }
    pub(crate) fn selected(&self, index: usize) -> Result<bool, Invalid> {
        self.selected
            .get(index)
            .copied()
            .ok_or(Invalid("solo projection track is missing"))
    }
    pub(crate) fn muted(&self, index: usize, track: &hex_project::Track) -> Result<bool, Invalid> {
        let selected = self.selected(index)?;
        Ok(!track.soloed() && (track.muted() || (self.any && !selected)))
    }
    pub(crate) fn source_selected(
        &self,
        project: &hex_project::Project,
        source: hex_project::SourceId,
    ) -> Result<bool, Invalid> {
        for (index, contributor) in project.tracks().iter().enumerate() {
            if contributor.source_id() == Some(source) && self.selected(index)? {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

/// Validated per-track stereo coefficients shared by independent source paths.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PreparedTrackMix {
    pub(crate) channel_gain: Frame,
    pub(crate) effects: Option<hex_lattice::StereoMatrix>,
    pub(crate) muted: bool,
}
impl PreparedTrackMix {
    pub(crate) const fn identity() -> Self {
        Self {
            channel_gain: [1.0, 1.0],
            effects: None,
            muted: false,
        }
    }
    pub fn new(gain: f32, balance: f32, muted: bool) -> Result<Self, Invalid> {
        Self::with_effects(gain, balance, muted, None)
    }

    pub(crate) fn with_effects(
        gain: f32,
        balance: f32,
        muted: bool,
        effects: Option<hex_lattice::StereoMatrix>,
    ) -> Result<Self, Invalid> {
        if !gain.is_finite() || gain < 0.0 {
            return Err(Invalid("gain must be finite and nonnegative"));
        }
        if !balance.is_finite() || !(-1.0..=1.0).contains(&balance) {
            return Err(Invalid("balance must be finite and within -1..1"));
        }
        Ok(Self {
            channel_gain: [
                gain * (1.0 - balance.max(0.0)),
                gain * (1.0 + balance.min(0.0)),
            ],
            effects: effects.filter(|matrix| matrix.coefficients() != [[1.0, 0.0], [0.0, 1.0]]),
            muted,
        })
    }

    pub fn channel_gain(self) -> Frame {
        self.channel_gain
    }

    pub(crate) fn with_channel_gain(mut self, gain: Frame) -> Self {
        self.channel_gain = gain;
        self
    }

    pub fn muted(self) -> bool {
        self.muted
    }

    /// Add one source frame with the prepared per-channel coefficients.
    pub fn add_frame(self, destination: &mut Frame, source: Frame) {
        let processed = self.apply_frame(source);
        destination[0] += processed[0];
        destination[1] += processed[1];
    }

    /// Return this track's post-fader contribution for routing to sends.
    #[inline]
    pub(crate) fn apply_frame(self, source: Frame) -> Frame {
        if self.muted {
            return [0.0; 2];
        }
        let source = self.effects.map_or(source, |matrix| matrix.apply(source));
        [
            source[0] * self.channel_gain[0],
            source[1] * self.channel_gain[1],
        ]
    }

    pub(crate) fn apply_block(self, frames: &mut [Frame]) {
        if self.muted {
            frames.fill([0.0; 2]);
            return;
        }
        if let Some(matrix) = self.effects {
            matrix.apply_block(frames);
        }
        let [gl, gr] = self.channel_gain;
        if gl != 1.0 || gr != 1.0 {
            for frame in frames.iter_mut() {
                frame[0] *= gl;
                frame[1] *= gr;
            }
        }
    }

    pub(crate) fn with_absolute_gain(mut self, gain: f32) -> Self {
        self.channel_gain[0] *= gain;
        self.channel_gain[1] *= gain;
        self
    }

    pub(crate) fn add_captured(
        self,
        destination: &mut Frame,
        source: Frame,
        capture: Option<&mut Frame>,
    ) {
        let source = self.apply_frame(source);
        destination[0] += source[0];
        destination[1] += source[1];
        if let Some(capture) = capture {
            capture[0] += source[0];
            capture[1] += source[1];
        }
    }

    pub(crate) fn without_effects(self) -> Self {
        Self {
            effects: None,
            ..self
        }
    }
}

impl Track {
    /// Overlapping clips are summed in this prototype; Live overlap semantics are unverified.
    pub fn new(clips: Vec<Clip>, gain: f32, muted: bool) -> Result<Self, Invalid> {
        Self::with_balance(clips, gain, muted, 0.0)
    }
    /// Prepare validated original Hex balance coefficients for rendering.
    pub fn with_balance(
        clips: Vec<Clip>,
        gain: f32,
        muted: bool,
        balance: f32,
    ) -> Result<Self, Invalid> {
        Ok(Self {
            track_id: None,
            clip_index: clip_index::build(&clips, Clip::index_bounds),
            has_streamed_clips: clips.iter().any(Clip::is_streamed),
            clips,
            mix: PreparedTrackMix::new(gain, balance, muted)?,
            source_mix: PreparedTrackMix::new(gain, balance, muted)?,
            gain_envelope: None,
            instrument: None,
            instrument_position: 0,
            ordered_prefix: None,
            ordered_suffix: None,
            device_chain: std::sync::Arc::from([]),
            sends: Vec::new(),
            instrument_id: None,
            instrument_state: None,
            foreign_instrument: None,
            tuning_identity: Default::default(),
            instrument_bypassed: false,
            instrument_input_gain: 1.0,
            instrument_output_gain: 1.0,
            instrument_latency: Some(0),
            instrument_runtime_latency: Some(0),
            instrument_meter: Default::default(),
            source_id: None,
            source_receiver_index: None,
            source_audio_receiver: true,
            source_input_enabled: true,
            audio_content_enabled: true,
            mutable_instrument: false,
            mutable_looping_source: false,
            midi_source_index: None,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn with_audio_instrument_and_effects(
        clips: Vec<Clip>,
        gain: f32,
        muted: bool,
        balance: f32,
        instrument: Option<InstrumentTrack>,
        instrument_position: usize,
        effects: Option<hex_lattice::StereoMatrix>,
        ordered_prefix: Option<hex_lattice::StereoMatrix>,
        ordered_suffix: Option<hex_lattice::StereoMatrix>,
        device_chain: Vec<hex_lattice::PreparedRuntimeInstance>,
    ) -> Result<Self, Invalid> {
        let stateful = !device_chain.is_empty();
        let ordered = instrument_position != 0;
        let segmented =
            stateful || (ordered && (ordered_prefix.is_some() || ordered_suffix.is_some()));
        let mix_effects = if segmented { None } else { effects };
        let mix = PreparedTrackMix::with_effects(gain, balance, muted, mix_effects)?;
        let source_mix = if segmented {
            PreparedTrackMix::new(1.0, 0.0, false)?
        } else {
            mix
        };
        Ok(Self {
            track_id: None,
            clip_index: clip_index::build(&clips, Clip::index_bounds),
            has_streamed_clips: clips.iter().any(Clip::is_streamed),
            clips,
            mix,
            source_mix,
            gain_envelope: None,
            instrument,
            instrument_position,
            ordered_prefix,
            ordered_suffix,
            device_chain: std::sync::Arc::from(device_chain),
            sends: Vec::new(),
            instrument_id: None,
            instrument_state: None,
            foreign_instrument: None,
            tuning_identity: Default::default(),
            instrument_bypassed: false,
            instrument_input_gain: 1.0,
            instrument_output_gain: 1.0,
            instrument_latency: Some(0),
            instrument_runtime_latency: Some(0),
            instrument_meter: Default::default(),
            source_id: None,
            source_receiver_index: None,
            source_audio_receiver: true,
            source_input_enabled: true,
            audio_content_enabled: true,
            mutable_instrument: false,
            mutable_looping_source: false,
            midi_source_index: None,
        })
    }

    pub(crate) fn with_instrument_config(
        mut self,
        instrument: Option<&hex_project::Device>,
    ) -> Self {
        self.instrument_id = instrument.map(hex_project::Device::id);
        self.instrument_state =
            instrument.map(|device| device.state().clone().without_host_gains());
        self.instrument_bypassed = instrument.is_some_and(hex_project::Device::bypassed);
        self.instrument_input_gain =
            instrument.map_or(1.0, |device| device.state().input_gain_linear());
        self.instrument_output_gain =
            instrument.map_or(1.0, |device| device.state().output_gain_linear());
        if self.instrument_output_gain != 1.0 || self.instrument_input_gain != 1.0 {
            self.source_mix = PreparedTrackMix {
                channel_gain: [1.0; 2],
                effects: None,
                muted: false,
            };
        }
        self
    }

    pub(crate) fn with_instrument_latency(
        mut self,
        source: Option<u32>,
        runtime: Option<u32>,
    ) -> Self {
        self.instrument_latency = source;
        self.instrument_runtime_latency = runtime;
        self
    }
    pub(crate) fn with_tuning_identity(mut self, tuning: shared_analytic::TuningIdentity) -> Self {
        self.tuning_identity = tuning;
        self
    }
    pub(crate) fn instrument_config_matches(&self, other: &Self) -> bool {
        self.tuning_identity == other.tuning_identity
            && self.instrument_id == other.instrument_id
            && self.instrument_state == other.instrument_state
            && self.instrument_bypassed == other.instrument_bypassed
    }

    /// A contributor retains its MIDI bank and local audio/effects, but only the
    /// selected receiver may render a retained source's generator output.
    pub(crate) fn with_source_audio_receiver(mut self, receiver: bool) -> Self {
        self.source_audio_receiver = receiver;
        self
    }
    pub(crate) fn with_source_mix_scope(
        mut self,
        input_enabled: bool,
        audio_enabled: bool,
    ) -> Self {
        self.source_input_enabled = input_enabled;
        self.audio_content_enabled = audio_enabled;
        self
    }
    pub(crate) fn source_input_enabled(&self) -> bool {
        self.source_input_enabled
    }
    pub(crate) fn audio_content_enabled(&self) -> bool {
        self.audio_content_enabled
    }
    pub fn receives_source_audio(&self) -> bool {
        self.source_audio_receiver
    }
    pub(crate) fn with_source_binding(
        mut self,
        source: Option<hex_project::SourceId>,
        receiver_index: Option<usize>,
    ) -> Self {
        self.source_id = source;
        self.source_receiver_index = receiver_index;
        self
    }
    pub fn source_id(&self) -> Option<hex_project::SourceId> {
        self.source_id
    }
    pub(crate) fn source_receiver_index(&self) -> Option<usize> {
        self.source_receiver_index
    }

    pub(crate) fn with_mutable_instrument(mut self, mutable: bool, looping_source: bool) -> Self {
        if mutable {
            // Device effects were prepared before the instrument capability
            // was attached. A gap-zero stateless chain may therefore still be
            // fused into the source/mix path; ordered rendering owns every
            // effect application once the mutable instrument is admitted.
            self.mix = self.mix.without_effects();
            self.source_mix = PreparedTrackMix::identity();
        }
        self.mutable_instrument = mutable;
        self.mutable_looping_source = mutable && looping_source;
        self
    }

    pub(crate) fn has_mutable_instrument(&self) -> bool {
        self.mutable_instrument
    }

    pub(crate) fn instrument_enabled(&self) -> bool {
        self.instrument_id.is_some() && !self.instrument_bypassed && self.instrument.is_some()
    }

    pub(crate) fn instrument_slot_enabled(&self) -> bool {
        self.instrument_id.is_some() && !self.instrument_bypassed
    }

    pub(crate) fn new_device_runtime(
        &self,
    ) -> Result<hex_lattice::NativeRuntimeChain, hex_lattice_contract::DeviceContractError> {
        let mut instances = Vec::new();
        instances
            .try_reserve_exact(self.device_chain.len())
            .map_err(|_| hex_lattice_contract::DeviceContractError::RuntimeAllocation)?;
        instances.extend(self.device_chain.iter().cloned());
        hex_lattice::NativeRuntimeChain::new_prepared(instances)
    }

    pub(crate) fn device_runtime_storage_preflight_bytes(&self) -> Option<usize> {
        hex_lattice::NativeRuntimeChain::declared_storage_preflight_bytes_for_prepared(
            &self.device_chain,
        )
    }

    pub(crate) fn with_sends(mut self, sends: Vec<CompiledSend>) -> Self {
        self.sends = sends;
        if !self.sends.is_empty() {
            self.source_mix = PreparedTrackMix::identity();
        }
        self
    }

    pub(crate) fn sends(&self) -> &[CompiledSend] {
        &self.sends
    }

    pub(crate) fn has_device_chain(&self) -> bool {
        !self.device_chain.is_empty()
    }

    pub(crate) fn stateful_device_count(&self) -> usize {
        self.device_chain
            .iter()
            .filter(|device| device.definition.is_some())
            .count()
    }

    pub(crate) fn stateful_device_declared_bytes(&self) -> Option<usize> {
        self.device_chain
            .iter()
            .filter_map(|device| device.definition.as_ref())
            .try_fold(0_usize, |total, definition| {
                total.checked_add(definition.declared_pair_bytes()?)
            })
    }

    pub(crate) fn instrument_position(&self) -> usize {
        self.instrument_position
    }

    pub(crate) fn has_ordered_instrument(&self) -> bool {
        self.instrument_position != 0
    }

    pub(crate) fn requires_ordered_processing(&self) -> bool {
        (self.instrument_id.is_some()
            && (self.instrument_output_gain != 1.0 || self.instrument_input_gain != 1.0))
            || self.mutable_instrument
            || self.midi_source_index.is_some()
            || self.has_device_chain()
            || (self.has_ordered_instrument()
                && (self.ordered_prefix.is_some() || self.ordered_suffix.is_some()))
    }

    pub(crate) fn apply_instrument_input_gain(&self, frames: &mut [Frame]) {
        if !self.instrument_bypassed && self.instrument_input_gain != 1.0 {
            for frame in frames {
                frame[0] *= self.instrument_input_gain;
                frame[1] *= self.instrument_input_gain;
            }
        }
    }
    pub(crate) fn apply_instrument_output_gain(&self, frames: &mut [Frame]) {
        if !self.instrument_bypassed {
            let gain = self.instrument_output_gain;
            if gain != 1.0 {
                for frame in frames {
                    frame[0] *= gain;
                    frame[1] *= gain;
                }
            }
        }
    }

    pub(crate) fn apply_ordered_prefix(&self, frames: &mut [Frame]) {
        if let Some(matrix) = self.ordered_prefix {
            matrix.apply_block(frames);
        }
    }

    pub(crate) fn apply_ordered_suffix(&self, frames: &mut [Frame]) {
        if let Some(matrix) = self.ordered_suffix {
            matrix.apply_block(frames);
        }
    }

    pub(crate) fn instrument_suffix_matrix(&self) -> Option<hex_lattice::StereoMatrix> {
        self.ordered_suffix
    }

    pub(crate) fn instrument_suffix_needs_tail(&self) -> bool {
        if self.has_ordered_instrument() {
            self.ordered_suffix.is_some()
                || self
                    .device_chain
                    .get(self.instrument_position..)
                    .is_some_and(|effects| {
                        effects.iter().any(|effect| {
                            effect.definition.is_some()
                                || !effect.automation.is_empty()
                                || effect.matrix.coefficients() != [[1.0, 0.0], [0.0, 1.0]]
                        })
                    })
        } else {
            self.has_device_chain()
        }
    }

    pub(crate) fn with_gain_envelope(
        mut self,
        lane: &hex_project::GainLane,
        tempo: &hex_project::TempoMap,
        balance: f32,
        fallback: f32,
    ) -> Result<Self, Invalid> {
        let neutral =
            PreparedTrackMix::with_effects(1.0, balance, self.mix.muted, self.mix.effects)?;
        self.gain_envelope =
            gain_envelope::PreparedGainEnvelope::prepare(lane, tempo, neutral, fallback)?;
        if self.gain_envelope.is_some() {
            self.source_mix = PreparedTrackMix::identity();
        }
        Ok(self)
    }
    /// Read-only prepared curve metadata. Empty means static fallback; this is not an applied-value acknowledgment.
    pub fn gain_envelope_knots(&self) -> &[GainEnvelopeKnot] {
        self.gain_envelope
            .as_ref()
            .map_or(&[], |envelope| envelope.knots())
    }
    /// Logical prepared gain at an Arrangement frame, independent of the runtime source owner.
    pub fn prepared_gain_at(&self, frame: u64) -> Option<f32> {
        self.gain_envelope
            .as_ref()
            .map(|envelope| envelope.value_at(frame))
    }
    pub(crate) fn arm_gain_recording(
        &mut self,
        fallback: f32,
        balance: f32,
    ) -> Result<(), Invalid> {
        if self.gain_envelope.is_none() {
            let neutral =
                PreparedTrackMix::with_effects(1.0, balance, self.mix.muted, self.mix.effects)?;
            self.gain_envelope = Some(gain_envelope::PreparedGainEnvelope::constant(
                neutral, fallback,
            ));
        }
        self.source_mix = PreparedTrackMix::identity();
        Ok(())
    }
    pub(crate) fn recording_mix(&self, gain: f32) -> PreparedTrackMix {
        self.gain_envelope
            .as_ref()
            .map_or(self.mix, |envelope| envelope.mix_with_gain(gain))
    }
    pub(crate) fn has_gain_automation(&self) -> bool {
        self.gain_envelope.is_some()
    }
    pub(crate) fn mix_at(&self, frame: u64) -> PreparedTrackMix {
        self.gain_envelope
            .as_ref()
            .map_or(self.mix, |envelope| envelope.mix_at(frame))
    }
    pub(crate) fn gain_touch_target_at(&self, frame: u64) -> Option<gain_envelope::TouchTarget> {
        self.gain_envelope.as_ref()?.touch_target_at(frame)
    }
    pub(crate) fn base_gain_at(&self, frame: u64) -> Option<f32> {
        self.gain_envelope
            .as_ref()
            .map(|envelope| envelope.base_value_at(frame))
    }
    pub(crate) fn read_gain_at(&self, frame: u64) -> Option<f32> {
        self.gain_envelope
            .as_ref()
            .map(|envelope| envelope.value_at(frame))
    }
    pub fn mix(&self) -> PreparedTrackMix {
        self.mix
    }

    pub(crate) fn source_mix(&self) -> PreparedTrackMix {
        self.source_mix
    }

    pub(crate) fn has_instrument_source(&self) -> bool {
        self.instrument.is_some() || self.foreign_instrument.is_some()
    }

    pub(crate) fn with_midi_source_index(mut self, source: Option<usize>) -> Self {
        self.midi_source_index = source;
        if source.is_some() {
            // Audio keeps its local owner; the block renderer adds the selected
            // MIDI bus at the destination instrument and applies its fader once.
            self.source_mix = PreparedTrackMix::identity();
        }
        self
    }

    pub(crate) fn with_foreign_instrument(
        mut self,
        node: Option<hex_lattice::PreparedRuntimeInstance>,
    ) -> Self {
        self.foreign_instrument = node;
        self
    }

    pub(crate) fn with_track_id(mut self, track: hex_project::TrackId) -> Self {
        self.track_id = Some(track);
        self
    }

    pub(crate) fn canonical_track_id(&self) -> Option<hex_project::TrackId> {
        self.track_id
    }

    /// Add only this track's finite Arrangement source to an existing mix buffer.
    /// `start_frame` anchors output[0]; transport is not changed. The range end is
    /// checked before any samples are added, so overflow leaves output untouched.
    pub fn render_additive(&self, start_frame: u64, output: &mut [Frame]) -> Result<(), Invalid> {
        if self.foreign_instrument.is_some() {
            return Err(Invalid("vst3 instrument render owner is unavailable"));
        }
        self.require_volume_timing(None)?;
        if self.has_gain_automation() {
            return Err(Invalid("automated track requires project rendering"));
        }
        if self.has_streamed_clips {
            return Err(Invalid("streamed clips require a media page provider"));
        }
        self.render_additive_with_midi_floor(start_frame, output, None, None)
    }

    pub(crate) fn require_volume_timing(
        &self,
        timing: Option<&dyn prepared_tempo::MidiTempoBreaks>,
    ) -> Result<(), Invalid> {
        if timing.is_none()
            && self
                .instrument
                .as_ref()
                .is_some_and(|i| i.volumes.is_some())
        {
            return Err(Invalid("clip volume requires project rendering timing"));
        }
        Ok(())
    }

    pub(crate) fn render_additive_with_midi_floor(
        &self,
        start_frame: u64,
        output: &mut [Frame],
        midi_onset_floor_exclusive: Option<u64>,
        timing: Option<&dyn prepared_tempo::MidiTempoBreaks>,
    ) -> Result<(), Invalid> {
        self.require_volume_timing(timing)?;
        let length = u64::try_from(output.len()).map_err(|_| Invalid("render length overflow"))?;
        let end_frame = start_frame
            .checked_add(length)
            .ok_or(Invalid("render range overflow"))?;
        if !self.source_mix.muted {
            self.render_unmuted_range(
                start_frame,
                end_frame,
                output,
                midi_onset_floor_exclusive,
                timing,
            )?;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn render_additive_with_suppressions_and_pages(
        &self,
        start_frame: u64,
        performance_frame: u64,
        output: &mut [Frame],
        suppressions: &[midi_template::MidiOccurrenceSuppression],
        pages: Option<&mut (impl MediaPages + ?Sized)>,
        continuations: &[Option<hex_lattice::AnalyticContinuation>],
        timing: Option<&dyn prepared_tempo::MidiTempoBreaks>,
        interactive_onset_floor: Option<crate::midi_compile::InteractiveOnsetFloor>,
    ) -> Result<(), Invalid> {
        self.require_volume_timing(timing)?;
        let length = u64::try_from(output.len()).map_err(|_| Invalid("render length overflow"))?;
        let end_frame = start_frame
            .checked_add(length)
            .ok_or(Invalid("render range overflow"))?;
        if !self.source_mix.muted {
            if let Some(page_provider) = pages {
                self.render_audio_range_with_pages(start_frame, end_frame, output, page_provider);
            } else {
                self.render_audio_range(start_frame, end_frame, output);
            }
            self.render_instrument_range_with_suppressions(
                (start_frame, end_frame),
                output,
                None,
                performance_frame,
                suppressions,
                None,
                continuations,
                timing,
                interactive_onset_floor,
            )?;
        }
        Ok(())
    }

    pub(crate) fn render_additive_with_pages(
        &self,
        start_frame: u64,
        output: &mut [Frame],
        pages: &mut (impl MediaPages + ?Sized),
        timing: Option<&dyn prepared_tempo::MidiTempoBreaks>,
    ) -> Result<(), Invalid> {
        self.require_volume_timing(timing)?;
        let length = u64::try_from(output.len()).map_err(|_| Invalid("render length overflow"))?;
        let end_frame = start_frame
            .checked_add(length)
            .ok_or(Invalid("render range overflow"))?;
        if !self.source_mix.muted {
            self.render_unmuted_range_with_pages(
                start_frame,
                end_frame,
                output,
                None,
                pages,
                timing,
            )?;
        }
        Ok(())
    }

    pub(crate) fn render_audio_additive(
        &self,
        start_frame: u64,
        output: &mut [Frame],
    ) -> Result<(), Invalid> {
        let length = u64::try_from(output.len()).map_err(|_| Invalid("render length overflow"))?;
        let end_frame = start_frame
            .checked_add(length)
            .ok_or(Invalid("render range overflow"))?;
        self.render_audio_range(start_frame, end_frame, output);
        Ok(())
    }

    // The direct Main fast path taps each existing source addition once. It
    // does not regroup sources into a track buffer (which changes rounding).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn render_additive_captured<P: MediaPages + ?Sized>(
        &self,
        start_frame: u64,
        performance_frame: u64,
        output: &mut [Frame],
        suppressions: &[midi_template::MidiOccurrenceSuppression],
        pages: Option<&mut P>,
        capture: &mut [Frame],
        continuations: &[Option<hex_lattice::AnalyticContinuation>],
        timing: Option<&dyn prepared_tempo::MidiTempoBreaks>,
        loop_fade: Option<loop_seam::LoopEndFade>,
        interactive_onset_floor: Option<crate::midi_compile::InteractiveOnsetFloor>,
    ) -> Result<(), Invalid> {
        self.require_volume_timing(timing)?;
        let end_frame = start_frame
            .checked_add(
                u64::try_from(output.len()).map_err(|_| Invalid("render length overflow"))?,
            )
            .ok_or(Invalid("render range overflow"))?;
        if capture.len() != output.len() {
            return Err(Invalid("capture length mismatch"));
        }
        if !self.source_mix.muted {
            if let Some(fade) = loop_fade.filter(|fade| fade.intersects(start_frame, output.len()))
            {
                if output.len() > 256 {
                    return Err(Invalid("captured source range exceeds render chunk"));
                }
                let mut audio = [[0.0; 2]; 256];
                let mut audio_capture = [[0.0; 2]; 256];
                let count = output.len();
                self.render_audio_range_captured(
                    start_frame,
                    end_frame,
                    &mut audio[..count],
                    pages,
                    Some(&mut audio_capture[..count]),
                );
                fade.apply(start_frame, &mut audio[..count]);
                fade.apply(start_frame, &mut audio_capture[..count]);
                for ((out, captured), (sample, tap)) in output
                    .iter_mut()
                    .zip(capture.iter_mut())
                    .zip(audio.iter().zip(audio_capture.iter()))
                {
                    out[0] += sample[0];
                    out[1] += sample[1];
                    captured[0] += tap[0];
                    captured[1] += tap[1];
                }
            } else {
                self.render_audio_range_captured(
                    start_frame,
                    end_frame,
                    output,
                    pages,
                    Some(capture),
                );
            }
            self.render_instrument_range_with_suppressions(
                (start_frame, end_frame),
                output,
                None,
                performance_frame,
                suppressions,
                Some(capture),
                continuations,
                timing,
                interactive_onset_floor,
            )?;
        }
        Ok(())
    }

    pub(crate) fn render_audio_additive_with_pages(
        &self,
        start_frame: u64,
        output: &mut [Frame],
        pages: &mut (impl MediaPages + ?Sized),
    ) -> Result<(), Invalid> {
        let length = u64::try_from(output.len()).map_err(|_| Invalid("render length overflow"))?;
        let end_frame = start_frame
            .checked_add(length)
            .ok_or(Invalid("render range overflow"))?;
        self.render_audio_range_with_pages(start_frame, end_frame, output, pages);
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn render_instrument_additive_with_suppressions(
        &self,
        start_frame: u64,
        performance_frame: u64,
        output: &mut [Frame],
        suppressions: &[midi_template::MidiOccurrenceSuppression],
        continuations: &[Option<hex_lattice::AnalyticContinuation>],
        timing: Option<&dyn prepared_tempo::MidiTempoBreaks>,
        interactive_onset_floor: Option<crate::midi_compile::InteractiveOnsetFloor>,
    ) -> Result<(), Invalid> {
        let length = u64::try_from(output.len()).map_err(|_| Invalid("render length overflow"))?;
        let end_frame = start_frame
            .checked_add(length)
            .ok_or(Invalid("render range overflow"))?;
        self.render_instrument_range_with_suppressions(
            (start_frame, end_frame),
            output,
            None,
            performance_frame,
            suppressions,
            None,
            continuations,
            timing,
            interactive_onset_floor,
        )
    }

    pub(crate) fn render_raw_audio<P: MediaPages + ?Sized>(
        &self,
        start: u64,
        output: &mut [Frame],
        pages: Option<&mut P>,
    ) -> Result<(), Invalid> {
        self.render_raw_audio_excluding(start, output, pages, [None; 2])
    }
    pub(crate) fn render_raw_audio_excluding<P: MediaPages + ?Sized>(
        &self,
        start: u64,
        output: &mut [Frame],
        pages: Option<&mut P>,
        excluded: [Option<u64>; 2],
    ) -> Result<(), Invalid> {
        let end = start
            .checked_add(output.len() as u64)
            .ok_or(Invalid("raw source range overflow"))?;
        self.render_audio_range_captured_with_mix(
            start,
            end,
            output,
            pages,
            None,
            PreparedTrackMix::identity(),
            excluded,
        );
        Ok(())
    }
    /// Frame coordinates are (Arrangement source start, performance start).
    pub(crate) fn render_raw_instrument(
        &self,
        frames: (u64, u64),
        output: &mut [Frame],
        suppressions: &[midi_template::MidiOccurrenceSuppression],
        continuations: &[Option<hex_lattice::AnalyticContinuation>],
        timing: Option<&dyn prepared_tempo::MidiTempoBreaks>,
        interactive_onset_floor: Option<crate::midi_compile::InteractiveOnsetFloor>,
    ) -> Result<(), Invalid> {
        let (start, performance) = frames;
        let end = start
            .checked_add(output.len() as u64)
            .ok_or(Invalid("raw instrument range overflow"))?;
        self.render_instrument_range_with_mix(
            (start, end),
            output,
            None,
            performance,
            suppressions,
            None,
            continuations,
            PreparedTrackMix::identity(),
            timing,
            interactive_onset_floor,
        )
    }
    /// Queries analytic source occurrences for reconstruction. Audible capture
    /// supplies its action's onset floor; structural/capability proofs pass None
    /// and remain conservative. Already-owned release ledgers are independent.
    pub(crate) fn for_each_active_midi_span(
        &self,
        frame: u64,
        interactive_onset_floor: Option<crate::midi_compile::InteractiveOnsetFloor>,
        mut visit: impl FnMut(&midi_compile::CompiledNoteSpan, midi_note_bank::PreparedMidiNote),
    ) -> Result<(), Invalid> {
        let end = frame
            .checked_add(1)
            .ok_or(Invalid("midi span frame overflow"))?;
        if let Some(instrument) = &self.instrument {
            instrument.for_each_intersecting(frame, end, None, |span| {
                if !span.admitted_at_interactive_start(interactive_onset_floor) {
                    return;
                }
                if let Some(sampler) = instrument.bank.sampler(span.sampler_index) {
                    visit(span, sampler);
                }
            });
        }
        Ok(())
    }

    pub(crate) fn for_each_mutable_midi_span(
        &self,
        start: u64,
        end: u64,
        mut visit: impl FnMut(&midi_compile::CompiledNoteSpan),
    ) -> Result<(), Invalid> {
        if start > end {
            return Err(Invalid("midi span frame range is invalid"));
        }
        if let Some(instrument) = self
            .instrument
            .as_ref()
            .filter(|instrument| instrument.mutable)
        {
            instrument.for_each_intersecting(start, end, None, &mut visit);
        }
        Ok(())
    }

    pub(crate) fn render_unmuted_range(
        &self,
        start_frame: u64,
        end_frame: u64,
        output: &mut [Frame],
        midi_onset_floor_exclusive: Option<u64>,
        timing: Option<&dyn prepared_tempo::MidiTempoBreaks>,
    ) -> Result<(), Invalid> {
        self.require_volume_timing(timing)?;
        self.render_audio_range(start_frame, end_frame, output);
        self.render_instrument_range(
            start_frame,
            end_frame,
            output,
            midi_onset_floor_exclusive,
            timing,
        )
    }

    pub(crate) fn render_audio_range(
        &self,
        start_frame: u64,
        end_frame: u64,
        output: &mut [Frame],
    ) {
        self.render_audio_range_captured::<dyn MediaPages>(
            start_frame,
            end_frame,
            output,
            None,
            None,
        );
    }

    pub(crate) fn render_audio_range_captured<P: MediaPages + ?Sized>(
        &self,
        start_frame: u64,
        end_frame: u64,
        output: &mut [Frame],
        pages: Option<&mut P>,
        capture: Option<&mut [Frame]>,
    ) {
        self.render_audio_range_captured_with_mix(
            start_frame,
            end_frame,
            output,
            pages,
            capture,
            self.source_mix,
            [None; 2],
        );
    }

    #[allow(clippy::too_many_arguments)] // Existing mix/capture sinks with one fixed occurrence exclusion.
    pub(crate) fn render_audio_range_captured_with_mix<P: MediaPages + ?Sized>(
        &self,
        start_frame: u64,
        end_frame: u64,
        output: &mut [Frame],
        pages: Option<&mut P>,
        mut capture: Option<&mut [Frame]>,
        source_mix: PreparedTrackMix,
        excluded: [Option<u64>; 2],
    ) {
        if let Some(pages) = pages {
            self.render_audio_range_with_pages_captured_with_mix(
                start_frame,
                end_frame,
                output,
                pages,
                capture,
                source_mix,
                excluded,
            );
            return;
        }
        clip_index::visit(
            &self.clip_index,
            &self.clips,
            start_frame,
            end_frame,
            |clip| {
                if !clip.is_streamed() && !excluded.contains(&Some(clip.start)) {
                    self.render_resident_clip(
                        clip,
                        start_frame,
                        end_frame,
                        output,
                        capture.as_deref_mut(),
                        source_mix,
                    );
                }
            },
        );
    }

    pub(crate) fn render_unmuted_range_with_pages(
        &self,
        start_frame: u64,
        end_frame: u64,
        output: &mut [Frame],
        midi_onset_floor_exclusive: Option<u64>,
        pages: &mut (impl MediaPages + ?Sized),
        timing: Option<&dyn prepared_tempo::MidiTempoBreaks>,
    ) -> Result<(), Invalid> {
        self.require_volume_timing(timing)?;
        self.render_audio_range_with_pages(start_frame, end_frame, output, pages);
        self.render_instrument_range(
            start_frame,
            end_frame,
            output,
            midi_onset_floor_exclusive,
            timing,
        )
    }

    pub(crate) fn render_audio_range_with_pages(
        &self,
        start_frame: u64,
        end_frame: u64,
        output: &mut [Frame],
        pages: &mut (impl MediaPages + ?Sized),
    ) {
        self.render_audio_range_with_pages_captured(start_frame, end_frame, output, pages, None);
    }

    pub(crate) fn render_audio_range_with_pages_captured(
        &self,
        start_frame: u64,
        end_frame: u64,
        output: &mut [Frame],
        pages: &mut (impl MediaPages + ?Sized),
        capture: Option<&mut [Frame]>,
    ) {
        self.render_audio_range_with_pages_captured_with_mix(
            start_frame,
            end_frame,
            output,
            pages,
            capture,
            self.source_mix,
            [None; 2],
        );
    }

    #[allow(clippy::too_many_arguments)] // Same track audio mix plus the borrowed page provider.
    pub(crate) fn render_audio_range_with_pages_captured_with_mix(
        &self,
        start_frame: u64,
        end_frame: u64,
        output: &mut [Frame],
        pages: &mut (impl MediaPages + ?Sized),
        mut capture: Option<&mut [Frame]>,
        source_mix: PreparedTrackMix,
        excluded: [Option<u64>; 2],
    ) {
        clip_index::visit(
            &self.clip_index,
            &self.clips,
            start_frame,
            end_frame,
            |clip| {
                if excluded.contains(&Some(clip.start)) {
                    return;
                }
                if !clip.is_streamed() {
                    self.render_resident_clip(
                        clip,
                        start_frame,
                        end_frame,
                        output,
                        capture.as_deref_mut(),
                        source_mix,
                    );
                    return;
                }
                clip.visit_streamed_spans(start_frame, end_frame, |source, span| {
                    render_streamed_page_span_captured(
                        source_mix,
                        source,
                        span,
                        output,
                        pages,
                        capture.as_deref_mut(),
                    )
                });
            },
        );
    }

    pub(crate) fn render_resident_clip(
        &self,
        clip: &Clip,
        start_frame: u64,
        end_frame: u64,
        output: &mut [Frame],
        mut capture: Option<&mut [Frame]>,
        source_mix: PreparedTrackMix,
    ) {
        if !clip.active {
            return;
        }
        let mut span_end = end_frame.min(clip.end);
        if !clip.looping {
            span_end = span_end.min(clip.start.saturating_add(clip.source_length as u64));
        }
        let span_start = start_frame.max(clip.start);
        if span_start >= span_end {
            return;
        }
        let output_start = (span_start - start_frame) as usize;
        let output_end = (span_end - start_frame) as usize;
        let metadata = clip.metadata.as_deref();
        let source_phase = metadata.map_or(0, |metadata| metadata.source_phase);
        let clip_gain = metadata.map_or(1.0, |metadata| metadata.gain);
        let fade = metadata.and_then(|metadata| metadata.fade.as_ref());
        if let Some(map) = metadata.and_then(|metadata| metadata.sparse.as_ref())
            && let Some(audio) = clip.audio.as_ref()
        {
            let first = clip.source_offset + (span_start - clip.start) as usize;
            map.visit(first..first + output_end - output_start, |root, packed| {
                let from = output_start + (root.start - first);
                for (offset, sample) in audio.frames()[packed].iter().copied().enumerate() {
                    let index = from + offset;
                    let mut sample = sample;
                    if let Some(fade) = fade {
                        let position = span_start + (index - output_start) as u64;
                        let gain = fade.gain_at(position - clip.start);
                        sample[0] *= gain;
                        sample[1] *= gain;
                    }
                    source_mix.add_captured(
                        &mut output[index],
                        sample.map(|channel| channel * clip_gain),
                        capture
                            .as_deref_mut()
                            .and_then(|frames| frames.get_mut(index)),
                    );
                }
            });
            return;
        }
        if fade.is_none()
            && clip_gain == 1.0
            && capture.is_none()
            && clip.render_neutral_additive(
                span_start,
                source_phase,
                &mut output[output_start..output_end],
                source_mix,
            )
        {
            return;
        }
        if fade.is_some() || clip_gain != 1.0 {
            for (offset, frame) in output[output_start..output_end].iter_mut().enumerate() {
                let position = span_start + offset as u64;
                let mut sample = clip.at(position, source_phase);
                if let Some(fade) = fade {
                    let gain = fade.gain_at(position - clip.start);
                    sample[0] *= gain;
                    sample[1] *= gain;
                }
                if clip_gain != 1.0 {
                    sample[0] *= clip_gain;
                    sample[1] *= clip_gain;
                }
                source_mix.add_captured(
                    frame,
                    sample,
                    capture
                        .as_deref_mut()
                        .and_then(|frames| frames.get_mut(output_start + offset)),
                );
            }
        } else if source_phase == 0 {
            for (offset, frame) in output[output_start..output_end].iter_mut().enumerate() {
                source_mix.add_captured(
                    frame,
                    clip.at(span_start + offset as u64, 0),
                    capture
                        .as_deref_mut()
                        .and_then(|frames| frames.get_mut(output_start + offset)),
                );
            }
        } else {
            for (offset, frame) in output[output_start..output_end].iter_mut().enumerate() {
                source_mix.add_captured(
                    frame,
                    clip.at(span_start + offset as u64, source_phase),
                    capture
                        .as_deref_mut()
                        .and_then(|frames| frames.get_mut(output_start + offset)),
                );
            }
        }
    }

    pub(crate) fn render_instrument_range(
        &self,
        start_frame: u64,
        end_frame: u64,
        output: &mut [Frame],
        midi_onset_floor_exclusive: Option<u64>,
        timing: Option<&dyn prepared_tempo::MidiTempoBreaks>,
    ) -> Result<(), Invalid> {
        self.render_instrument_range_with_suppressions(
            (start_frame, end_frame),
            output,
            midi_onset_floor_exclusive,
            0,
            &[],
            None,
            &[],
            timing,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn render_instrument_range_with_suppressions(
        &self,
        (start_frame, end_frame): (u64, u64),
        output: &mut [Frame],
        midi_onset_floor_exclusive: Option<u64>,
        performance_frame: u64,
        suppressions: &[midi_template::MidiOccurrenceSuppression],
        capture: Option<&mut [Frame]>,
        continuations: &[Option<hex_lattice::AnalyticContinuation>],
        timing: Option<&dyn prepared_tempo::MidiTempoBreaks>,
        interactive_onset_floor: Option<crate::midi_compile::InteractiveOnsetFloor>,
    ) -> Result<(), Invalid> {
        self.render_instrument_range_with_mix(
            (start_frame, end_frame),
            output,
            midi_onset_floor_exclusive,
            performance_frame,
            suppressions,
            capture,
            continuations,
            self.source_mix,
            timing,
            interactive_onset_floor,
        )
    }

    #[allow(clippy::too_many_arguments)] // Existing source render plus explicit gap-stage mix.
    pub(crate) fn render_instrument_range_with_mix(
        &self,
        (start_frame, end_frame): (u64, u64),
        output: &mut [Frame],
        midi_onset_floor_exclusive: Option<u64>,
        performance_frame: u64,
        suppressions: &[midi_template::MidiOccurrenceSuppression],
        mut capture: Option<&mut [Frame]>,
        continuations: &[Option<hex_lattice::AnalyticContinuation>],
        source_mix: PreparedTrackMix,
        timing: Option<&dyn prepared_tempo::MidiTempoBreaks>,
        interactive_onset_floor: Option<crate::midi_compile::InteractiveOnsetFloor>,
    ) -> Result<(), Invalid> {
        self.require_volume_timing(timing)?;
        if let Some(instrument) = &self.instrument {
            if instrument.mutable {
                return Ok(());
            }
            for chunk_start in (start_frame..end_frame).step_by(256) {
                let chunk_end = chunk_start.saturating_add(256).min(end_frame);
                let output_start = (chunk_start - start_frame) as usize;
                let output_end = (chunk_end - start_frame) as usize;
                let target = &mut output[output_start..output_end];
                let chunk_performance_frame = performance_frame
                    .checked_add(chunk_start - start_frame)
                    .ok_or(Invalid("midi performance frame overflow"))?;
                let mut failure = None;
                instrument.for_each_intersecting(
                    chunk_start,
                    chunk_end,
                    midi_onset_floor_exclusive,
                    |span| {
                        if !span.admitted_at_interactive_start(interactive_onset_floor) {
                            return;
                        }
                        let carried =
                            self.track_id.is_some_and(|track| {
                                suppressions
                                    .binary_search_by(|entry| {
                                        (entry.track, entry.clip, entry.note, entry.iteration)
                                            .cmp(&(track, span.clip, span.note, span.iteration))
                                    })
                                    .ok()
                                    .and_then(|index| suppressions.get(index))
                                    .is_some_and(|entry| {
                                        entry.until_frame > chunk_performance_frame
                                    })
                            });
                        if carried {
                            return;
                        }
                        let start = span.start.max(chunk_start);
                        let end = span.end.min(chunk_end);
                        let Some(voice) = instrument.bank.sampler(span.sampler_index) else {
                            return;
                        };
                        let volume = instrument.volumes.as_ref().and_then(|v| v.clip(span.clip));
                        let mut volume_ramp: Option<midi_clip_volume::VolumeRamp> = None;
                        for frame_position in start..end {
                            let gain = if let Some(volume) = volume {
                                if volume_ramp.is_none_or(|ramp| {
                                    ramp.next_boundary
                                        .is_some_and(|next| frame_position >= next)
                                }) {
                                    let Some(timing) = timing else {
                                        failure = Some(Invalid(
                                            "clip volume requires project rendering timing",
                                        ));
                                        return;
                                    };
                                    match volume.segment_or_hold(
                                        timing,
                                        frame_position,
                                        (end - frame_position) as usize,
                                    ) {
                                        Ok(ramp) => volume_ramp = Some(ramp),
                                        Err(error) => {
                                            failure = Some(error);
                                            return;
                                        }
                                    }
                                }
                                match volume_ramp.map(|ramp| ramp.gain_at(frame_position)) {
                                    Some(Ok(gain)) => gain,
                                    Some(Err(error)) => {
                                        failure = Some(error);
                                        return;
                                    }
                                    None => {
                                        failure = Some(Invalid("clip volume ramp is unavailable"));
                                        return;
                                    }
                                }
                            } else {
                                1.0
                            };
                            let relative = frame_position - span.start;
                            let Some(absolute) =
                                chunk_performance_frame.checked_add(frame_position - chunk_start)
                            else {
                                failure = Some(Invalid("midi performance frame overflow"));
                                return;
                            };
                            match voice.sample_at(
                                &instrument.shared_contexts,
                                &[],
                                continuations,
                                absolute,
                                relative,
                                span.gate_end - span.start,
                            ) {
                                Ok(mut sample) => {
                                    if gain != 1.0 {
                                        sample[0] = (f64::from(sample[0]) * gain) as f32;
                                        sample[1] = (f64::from(sample[1]) * gain) as f32;
                                    }
                                    source_mix.add_captured(
                                        &mut target[(frame_position - chunk_start) as usize],
                                        sample,
                                        capture.as_deref_mut().and_then(|frames| {
                                            frames.get_mut((frame_position - start_frame) as usize)
                                        }),
                                    );
                                }
                                Err(error) => {
                                    failure = Some(error);
                                    return;
                                }
                            }
                        }
                    },
                );
                if let Some(error) = failure {
                    return Err(error);
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub(crate) struct InstrumentTrack {
    pub(crate) spans: Vec<midi_compile::CompiledNoteSpan>,
    pub(crate) nodes: Vec<midi_compile::SpanNode>,
    pub(crate) root: Option<usize>,
    pub(crate) timeline_end: Option<u64>,
    pub(crate) bank: Arc<midi_note_bank::PreparedMidiNoteBank>,
    pub(crate) shared_contexts: Arc<shared_analytic::SharedAnalyticContextTable>,
    pub(crate) mutable: bool,
    pub(crate) volumes: Option<midi_clip_volume::arrangement::PreparedArrangementVolumes>,
}

impl InstrumentTrack {
    pub(crate) fn prepared_storage_bytes(&self) -> Result<usize, Invalid> {
        let mut bytes = self.bank.storage_bytes()?;
        if let Some(volumes) = &self.volumes {
            bytes = bytes
                .checked_add(volumes.allocated_row_bytes()?)
                .and_then(|n| n.checked_add(volumes.retained_control_metadata_bytes()))
                .ok_or(Invalid("prepared midi volume byte count overflow"))?;
        }
        Ok(bytes)
    }
    pub(crate) fn for_each_intersecting(
        &self,
        start: u64,
        end: u64,
        minimum_start_exclusive: Option<u64>,
        visit: impl FnMut(&midi_compile::CompiledNoteSpan),
    ) {
        midi_activity::visit_arrangement_spans(
            &self.spans,
            &self.nodes,
            self.root,
            start,
            end,
            minimum_start_exclusive,
            visit,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Audio, Clip};

    #[test]
    fn prepared_track_mix_validates_gain_pan_and_mute() {
        assert!(PreparedTrackMix::new(f32::NAN, 0.0, false).is_err());
        assert!(PreparedTrackMix::new(1.0, 2.0, false).is_err());
        let muted = PreparedTrackMix::new(1.0, 0.0, true).unwrap();
        let mut frame = [0.0, 0.0];
        muted.add_frame(&mut frame, [1.0, -1.0]);
        assert_eq!(frame, [0.0, 0.0]);

        let audio = Audio::new(48_000, vec![[0.5, -0.5]]).unwrap();
        let clip = Clip::new(audio, 0, 1, false).unwrap();
        let track = Track::new(vec![clip], 0.5, false).unwrap();
        assert_eq!(track.clips.len(), 1);
        assert_eq!(track.mix.channel_gain(), [0.5, 0.5]);
    }
}
