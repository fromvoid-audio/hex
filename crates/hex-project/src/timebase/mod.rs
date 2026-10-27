mod automation_projection;
mod input_grid;
pub use automation_projection::{
    AUTOMATION_PROJECTION_VERSION, CapturedAutomationProjection,
    visit_automation_projection_identity,
};
pub use input_grid::{InputQuantization, MusicalGridSegment, ResolvedInputGrid};
mod meter;
mod pulses;
mod tempo;
mod units;

pub use meter::{MeterMap, MeterPoint, MeterPosition};
pub use pulses::{BeatPulse, query_beat_pulses};
pub use tempo::{TempoMap, TempoPoint};
pub use tempo::{
    TempoMapCopyCustody, TempoMapCopyError, TempoMapCopyPhase, TempoMapOwnedAllocationLayout,
};
pub use units::{BeatTime, MAX_METER_POINTS, MAX_TEMPO_POINTS, Meter, Tempo};

pub use meter::{
    MeterMapCopyCustody, MeterMapCopyError, MeterMapCopyPhase, MeterMapOwnedAllocationLayout,
};
