//! Fresh sized retained-control TypedJob Box::new constructor qualification.
//! Environmental selection is shared; Sampler's constructor decision is not.
pub const FAMILY: &str = "fresh-retained-control-job-sized-box-new-v1";
pub fn decide(observed: super::pcm_gate::Observation<'_>) -> Result<(), &'static str> {
    super::pcm_gate::decide(observed)
}
