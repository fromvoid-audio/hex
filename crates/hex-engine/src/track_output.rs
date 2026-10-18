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
        } = self;
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
            instrument_id: None,
            instrument_state: None,
            tuning_identity: Default::default(),
            instrument_bypassed: false,
            instrument_input_gain: 1.0,
            instrument_output_gain: 1.0,
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
