//! Isolated source probe; this does not exercise a native audio callback.
#[path = "../../../hex-dsp/src/modulation_lfo.rs"]
mod modulation_lfo;

use modulation_lfo::{LfoRate, ModulationLfo};
use std::hint::black_box;

#[test]
fn rendering_reusable_source_samples_does_not_allocate_or_free() {
    let rate = LfoRate::tempo(48_000.0, 120.0, 4.0).unwrap();
    let mut source = ModulationLfo::new(rate, 0.0).unwrap();
    let mut samples = [0.0; 257];
    let info = allocation_counter::measure(|| {
        for _ in 0..32 {
            source.render(black_box(&mut samples));
            black_box(&samples);
        }
    });
    assert_eq!(info.count_total, 0);
    assert_eq!(info.bytes_total, 0);
    assert_eq!(info.count_current, 0);
    assert_eq!(info.bytes_current, 0);
}
