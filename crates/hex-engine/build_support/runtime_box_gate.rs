//! Separate sized SamplerRuntime Box::new constructor qualification.
//! Share environmental selection, never the PCM/MIDI constructor-family proof.
pub const FAMILY: &str = "sampler-runtime-sized-box-new-v1";
pub fn decide(observed: super::pcm_gate::Observation<'_>) -> Result<(), &'static str> {
    super::pcm_gate::decide(observed)
}
