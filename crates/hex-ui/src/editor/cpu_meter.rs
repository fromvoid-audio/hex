//! Delegates smoothing and overload hold to [`hex_ui_model::cpu_meter`].
pub(super) use hex_ui_model::cpu_meter::{CpuMeter, CpuSample};

pub(super) fn sample_from_audio(sample: hex_audio::cpu_meter::CpuSample) -> CpuSample {
    CpuSample {
        elapsed: sample.elapsed,
        budget: sample.budget,
        peak: sample.peak,
        frames: sample.frames,
        rate: sample.rate,
    }
}
