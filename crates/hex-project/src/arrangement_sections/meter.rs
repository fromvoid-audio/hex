//! Section transforms preserve authored positive-beat bar origins, but a cut is
//! not itself a meter change. Synthetic seams are retained only when the meter
//! differs from the preceding output state. Beat zero always supplies the initial
//! signature; moving/copying that required anchor does not author a new bar reset.
//! This provenance is local to constructing one edit, not a new saved field.
use crate::{BeatTime, Invalid, Meter, MeterMap, MeterPoint};
use std::collections::BTreeMap;

#[derive(Default)]
pub(super) struct MeterEdit {
    points: BTreeMap<BeatTime, (Meter, bool)>,
}
impl MeterEdit {
    pub(super) fn original(&mut self, at: BeatTime, source: MeterPoint) {
        self.insert(at, source.meter(), source.beat() != BeatTime::ZERO);
    }
    pub(super) fn seam(&mut self, at: BeatTime, meter: Meter) {
        self.insert(at, meter, false);
    }
    fn insert(&mut self, at: BeatTime, meter: Meter, authored: bool) {
        let keep_origin = authored
            || self
                .points
                .get(&at)
                .is_some_and(|(old, authored)| *authored && *old == meter);
        self.points.insert(at, (meter, keep_origin));
    }
    pub(super) fn finish(mut self, initial: Meter) -> Result<MeterMap, Invalid> {
        self.points
            .entry(BeatTime::ZERO)
            .or_insert((initial, false));
        let mut result = Vec::new();
        result
            .try_reserve_exact(self.points.len())
            .map_err(|_| Invalid("could not reserve section meter map"))?;
        for (beat, (meter, authored)) in self.points {
            if authored
                || result
                    .last()
                    .is_none_or(|previous: &MeterPoint| previous.meter() != meter)
            {
                result.push(MeterPoint::new(beat, meter));
            }
        }
        MeterMap::new(result)
    }
}
