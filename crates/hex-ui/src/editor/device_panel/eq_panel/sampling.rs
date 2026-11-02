//! Control-side authored response sampling; no live/audio ownership.
use super::EqState;

// Same 256-query limit as the registered response API and shared graph. All
// 64 authored bands retain a center and two shoulder samples, even at capacity.
const MAX_RESPONSE_POINTS: usize = 256;
const BASE_RESPONSE_POINTS: usize = 96;
pub(super) type Curves = [Vec<[f32; 2]>; 2];

pub(super) fn frequencies(state: &EqState, rate: u32) -> Vec<f32> {
    let upper = (rate as f32 * 0.49).min(20000.0);
    let active = state.nodes.iter().filter(|node| node.enabled).count();
    let base = BASE_RESPONSE_POINTS.min(MAX_RESPONSE_POINTS - 3 * active);
    let mut points = Vec::with_capacity(base + 3 * active);
    for i in 0..base {
        points.push((20.0 * ((upper / 20.0).ln() * i as f32 / (base - 1) as f32).exp()).min(upper));
    }
    for node in state.nodes.iter().filter(|node| node.enabled) {
        let center = node.frequency_hz.min(rate as f32 * 0.49);
        // Bounded shoulders make narrow authored features readable. These are
        // display samples, not a second filter/Q law or a uniform-error promise.
        let half_width = 0.5 / node.q;
        for f in [
            center * (-half_width).exp(),
            center,
            center * half_width.exp(),
        ] {
            points.push(f.clamp(20.0, upper));
        }
    }
    points.sort_unstable_by(f32::total_cmp);
    points.dedup();
    points
}

pub(super) fn curves(
    state: &EqState,
    rate: u32,
    sample: impl FnOnce(&[f32]) -> Option<Vec<[f32; 2]>>,
) -> Option<Curves> {
    let frequencies = frequencies(state, rate);
    let response = sample(&frequencies)?;
    Some(std::array::from_fn(|channel| {
        frequencies
            .iter()
            .zip(&response)
            .map(|(frequency, m)| {
                [
                    super::axes::x(*frequency, rate),
                    ((m[channel].max(0.00001).log10() * 20.0 + 24.0) / 48.0).clamp(0.0, 1.0),
                ]
            })
            .collect()
    }))
}
