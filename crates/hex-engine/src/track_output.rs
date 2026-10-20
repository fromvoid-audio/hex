//! Shared mix/curve preflight and allocation-free Track field assembly.
//!
//! This carries no constructor provenance or storage permit. The closed native
//! caller must retain original paired child declarations in actual custody;
//! legacy compilation can use the same musical layout with unqualified owners.
use crate::{Clip, InstrumentTrack, Invalid, PreparedEffectInstance, PreparedTrackMix, Track};
use hex_lattice::{DeviceMeter, StereoMatrix};
use std::sync::Arc;

pub(crate) struct CompiledTrackMix {
    mix: PreparedTrackMix,
    source_mix: PreparedTrackMix,
    instrument_position: usize,
    ordered_prefix: Option<StereoMatrix>,
    ordered_suffix: Option<StereoMatrix>,
    gain_envelope: Option<crate::gain_envelope::PreparedGainEnvelope>,
    instrument_metadata: Option<CompiledInstrumentMetadata>,
}
struct CompiledInstrumentMetadata {
    id: hex_project::DeviceId,
    state: hex_lattice_contract::NativeDeviceState,
    bypassed: bool,
    input_gain: f32,
    output_gain: f32,
}
impl CompiledTrackMix {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn prepare(
        gain: f32,
        muted: bool,
        balance: f32,
        instrument_position: usize,
        effects: Option<StereoMatrix>,
        ordered_prefix: Option<StereoMatrix>,
        ordered_suffix: Option<StereoMatrix>,
        stateful: bool,
    ) -> Result<Self, Invalid> {
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
            mix,
            source_mix,
            instrument_position,
            ordered_prefix,
            ordered_suffix,
            gain_envelope: None,
            instrument_metadata: None,
        })
    }
    /// Finish fallible curve preparation before final Track child ownership
    /// moves. Gain Vec backings are real construction allocations, retained by
    /// this plan until assembly. No shared meter/chain birth, owning clone,
    /// provider callback or publication authority is introduced here.
    pub(crate) fn prepare_gain_envelope(
        mut self,
        lane: &hex_project::GainLane,
        tempo: &hex_project::TempoMap,
        balance: f32,
        fallback: f32,
    ) -> Result<Self, Invalid> {
        let neutral =
            PreparedTrackMix::with_effects(1.0, balance, self.mix.muted, self.mix.effects)?;
        self.gain_envelope =
            crate::gain_envelope::PreparedGainEnvelope::prepare(lane, tempo, neutral, fallback)?;
        if self.gain_envelope.is_some() {
            self.source_mix = PreparedTrackMix::identity();
        }
        Ok(self)
    }
    /// Capture the actual canonical state alias before final assembly. This
    /// clone is a physical owner destination to include in construction custody,
    /// not a factory/configuration provenance upgrade. Assembly only moves it.
    pub(crate) fn prepare_instrument_metadata(
        mut self,
        instrument: Option<&hex_project::Device>,
    ) -> Self {
        self.instrument_metadata = instrument.map(|device| CompiledInstrumentMetadata {
            id: device.id(),
            state: device.state().clone().without_host_gains(),
            bypassed: device.bypassed(),
            input_gain: device.state().input_gain_linear(),
            output_gain: device.state().output_gain_linear(),
        });
        if self
            .instrument_metadata
            .as_ref()
            .is_some_and(|metadata| metadata.input_gain != 1.0 || metadata.output_gain != 1.0)
        {
            self.source_mix = PreparedTrackMix::identity();
        }
        self
    }
    /// Move the actual supplied children once. No allocation, cloning, provider
    /// call, qualification, refusal or replacement of a just-created child.
    pub(crate) fn assemble(
        self,
        clips: Vec<Clip>,
        instrument: Option<InstrumentTrack>,
        device_chain: Arc<[PreparedEffectInstance]>,
        instrument_meter: Arc<DeviceMeter>,
    ) -> Track {
        let Self {
            mix,
            source_mix,
            instrument_position,
            ordered_prefix,
            ordered_suffix,
            gain_envelope,
            instrument_metadata,
        } = self;
        let (
            instrument_id,
            instrument_state,
            instrument_bypassed,
            instrument_input_gain,
            instrument_output_gain,
        ) = match instrument_metadata {
            Some(CompiledInstrumentMetadata {
                id,
                state,
                bypassed,
                input_gain,
                output_gain,
            }) => (Some(id), Some(state), bypassed, input_gain, output_gain),
            None => (None, None, false, 1.0, 1.0),
        };
        Track {
            track_id: None,
            clips,
            mix,
            source_mix,
            gain_envelope,
            instrument,
            instrument_position,
            ordered_prefix,
            ordered_suffix,
            device_chain,
            sends: Vec::new(),
            instrument_id,
            instrument_state,
            tuning_identity: Default::default(),
            instrument_bypassed,
            instrument_input_gain,
            instrument_output_gain,
            instrument_latency: Some(0),
            instrument_runtime_latency: Some(0),
            instrument_meter,
            mutable_instrument: false,
            mutable_looping_source: false,
            midi_source_index: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::owned_shared::OwnedShared;
    #[test]
    fn preflighted_instrument_metadata_moves_one_actual_alias_with_host_gains_and_bypass() {
        use hex_project::{Command, Created, Editor};
        for bypassed in [false, true] {
            for hosted in [false, true] {
                let mut editor =
                    Editor::new(hex_project::Project::new(48_000).unwrap(), 8).unwrap();
                let Created::Track(id) = editor
                    .execute(Command::AddMidiTrack {
                        name: "sampler".into(),
                    })
                    .unwrap()
                else {
                    panic!("track")
                };
                let device = editor
                    .project()
                    .track(id)
                    .unwrap()
                    .instrument()
                    .unwrap()
                    .id();
                let mut state = crate::empty_sampler_state_on_control().unwrap();
                if hosted {
                    state = state
                        .with_input_gain_db(-6.0)
                        .unwrap()
                        .with_output_gain_db(3.0)
                        .unwrap();
                }
                editor
                    .execute(Command::ReplaceDeviceState { device, state })
                    .unwrap();
                editor
                    .execute(Command::SetDeviceBypass { device, bypassed })
                    .unwrap();
                let source = editor.project().track(id).unwrap().instrument().unwrap();
                let before = source.state().payload_storage_observation();
                let pointer = source.state().payload().as_ptr();
                let plan = CompiledTrackMix::prepare(0.4, false, 0.25, 0, None, None, None, false)
                    .unwrap()
                    .prepare_instrument_metadata(Some(source));
                assert_eq!(source.state().payload_storage_observation().1, before.1 + 1);
                let chain = OwnedShared::<[PreparedEffectInstance]>::new_owned_empty_slice();
                let meter = OwnedShared::new_owned(DeviceMeter::default());
                let (chain, chain_birth) = chain.into_paired_parts_on_control();
                let (meter, meter_birth) = meter.into_paired_parts_on_control();
                crate::owned_shared::reset_birth_count();
                crate::owned_shared::reset_clone_count();
                let track = plan.assemble(Vec::new(), None, chain, meter);
                let retained = track.instrument_state.as_ref().unwrap();
                assert_eq!(retained.payload().as_ptr(), pointer);
                assert_eq!(retained.payload_storage_observation().1, before.1 + 1);
                assert_eq!(retained.input_gain_millidb(), 0);
                assert_eq!(retained.output_gain_millidb(), 0);
                assert_eq!(track.instrument_id, Some(device));
                assert_eq!(track.instrument_bypassed, bypassed);
                assert_eq!(
                    track.instrument_input_gain,
                    source.state().input_gain_linear()
                );
                assert_eq!(
                    track.instrument_output_gain,
                    source.state().output_gain_linear()
                );
                let expected_source_mix = if hosted { [1.0, -1.0] } else { [0.3, -0.4] };
                assert_eq!(
                    track.source_mix.apply_frame([1.0, -1.0]),
                    expected_source_mix
                );
                assert!(chain_birth.parts(&track.device_chain).is_some());
                assert!(meter_birth.parts(&track.instrument_meter).is_some());
                assert_eq!(crate::owned_shared::birth_count(), 0);
                assert_eq!(crate::owned_shared::clone_count(), 0);
                drop(track);
                assert_eq!(source.state().payload_storage_observation(), before);
                eprintln!(
                    "SUPPORTED preflighted instrument metadata bypassed={bypassed} hosted={hosted} one actual alias/move/retirement"
                );
            }
        }
    }
    #[test]
    fn preflighted_track_gain_moves_prepared_backing_without_shared_birth_or_clone() {
        use hex_project::{BeatTime, Command, Created, Editor, GainInterpolation, GainPoint};
        let mut editor = Editor::new(hex_project::Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Track(id) = editor
            .execute(Command::AddMidiTrack {
                name: "gain".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        editor
            .execute(Command::ReplaceTrackGainEnvelope {
                track: id,
                points: vec![GainPoint {
                    beat: BeatTime::ZERO,
                    gain: 0.75,
                    outgoing: GainInterpolation::Step,
                }],
            })
            .unwrap();
        let source = editor.project().track(id).unwrap();
        let chain = OwnedShared::<[PreparedEffectInstance]>::new_owned_empty_slice();
        let meter = OwnedShared::new_owned(DeviceMeter::default());
        let (chain, chain_birth) = chain.into_paired_parts_on_control();
        let (meter, meter_birth) = meter.into_paired_parts_on_control();
        crate::owned_shared::reset_birth_count();
        crate::owned_shared::reset_clone_count();
        let plan = CompiledTrackMix::prepare(0.4, false, 0.25, 0, None, None, None, false)
            .unwrap()
            .prepare_gain_envelope(source.gain_lane(), editor.project().tempo_map(), 0.25, 0.4)
            .unwrap();
        let knots = plan.gain_envelope.as_ref().unwrap().knots().as_ptr();
        let track = plan.assemble(Vec::new(), None, chain, meter);
        assert_eq!(track.gain_envelope_knots().as_ptr(), knots);
        assert_eq!(track.prepared_gain_at(0), Some(0.75));
        assert_eq!(track.source_mix.apply_frame([1.0, -1.0]), [1.0, -1.0]);
        assert!(chain_birth.parts(&track.device_chain).is_some());
        assert!(meter_birth.parts(&track.instrument_meter).is_some());
        assert_eq!(crate::owned_shared::birth_count(), 0);
        assert_eq!(crate::owned_shared::clone_count(), 0);
        assert!(
            CompiledTrackMix::prepare(0.4, false, 0.25, 0, None, None, None, false)
                .unwrap()
                .prepare_gain_envelope(
                    source.gain_lane(),
                    editor.project().tempo_map(),
                    0.25,
                    f32::NAN
                )
                .is_err()
        );
        assert_eq!(crate::owned_shared::birth_count(), 0);
        eprintln!(
            "SUPPORTED preflighted gain actual backing/paired chain+meter move and invalid fallback refusal"
        );
    }
    #[test]
    fn preflighted_track_assembly_moves_actual_paired_children_without_replacement() {
        if !crate::audio_storage_constructor_layout_supported() {
            eprintln!("unsupported preflighted track field assembly");
            return;
        }
        eprintln!("SUPPORTED preflighted track field assembly actual paired empty chain/meter");
        let plan =
            CompiledTrackMix::prepare(0.5, false, -0.25, 0, None, None, None, false).unwrap();
        let chain = OwnedShared::<[PreparedEffectInstance]>::new_owned_empty_slice();
        let meter = OwnedShared::new_owned(DeviceMeter::default());
        let (chain, chain_birth) = chain.into_paired_parts_on_control();
        let (meter, meter_birth) = meter.into_paired_parts_on_control();
        let track = plan.assemble(Vec::new(), None, chain, meter);
        assert!(chain_birth.parts(&track.device_chain).is_some());
        assert!(meter_birth.parts(&track.instrument_meter).is_some());
        assert_eq!(Arc::strong_count(&track.device_chain), 1);
        assert_eq!(Arc::strong_count(&track.instrument_meter), 1);
        assert_eq!(track.mix.apply_frame([2.0, -4.0]), [1.0, -1.5]);
        assert_eq!(track.source_mix.apply_frame([2.0, -4.0]), [1.0, -1.5]);
        for gain in [f32::NAN, -1.0, f32::INFINITY] {
            assert!(
                CompiledTrackMix::prepare(gain, false, 0.0, 0, None, None, None, false).is_err()
            );
        }
    }
}
