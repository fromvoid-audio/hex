//! Shared scalar preflight and allocation-free Track field assembly.
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
        })
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
        } = self;
        Track {
            track_id: None,
            clips,
            mix,
            source_mix,
            gain_envelope: None,
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
