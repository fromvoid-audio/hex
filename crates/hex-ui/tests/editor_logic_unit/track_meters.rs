#[path = "../../src/editor/track_meters/selection.rs"]
mod selection;

#[expect(
    dead_code,
    reason = "the track collection owns seeded and unobserved meters"
)]
#[path = "../../src/editor/track_meters/decay.rs"]
mod decay;
#[allow(dead_code)]
#[path = "../../src/editor/track_meters/scale.rs"]
mod scale;
use decay::{StereoMeterState, decay_factor};
#[path = "../../src/editor/track_meters/follower.rs"]
mod follower;
