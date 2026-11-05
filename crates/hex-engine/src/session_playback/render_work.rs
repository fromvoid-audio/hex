//! Sample one Track/Return owner per render transaction, rotating in immutable
//! revision order. Only that owner reads the clock. Source preparation and its
//! local render/route processing are disjoint scopes; groups exclude children,
//! and shared instruments belong to their actual receiver. Main, detached old
//! tails, scheduling and other shared work remain in the global callback meter.
//! Foreign endpoint calls include their host cost, not asynchronous worker CPU.
//! Inline scalar scratch has no independently movable or destructible resources.
use crate::PreparedRevision;
use std::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Owner {
    Track(usize),
    Return(usize),
}

#[derive(Default)]
pub(super) struct Sampler {
    cursor: usize,
    target: Option<(Owner, u16)>,
    elapsed_ns: u64,
    scopes: usize,
}

impl Sampler {
    pub(super) fn begin(&mut self, revision: &PreparedRevision, metering: bool) {
        self.target = None;
        self.elapsed_ns = 0;
        self.scopes = 0;
        // Candidate/seek preparation can borrow this same immutable revision.
        // The audio publication owner enables metering only after preparation.
        if !metering {
            return;
        }
        let tracks = revision.arrangement().tracks();
        let returns = revision.arrangement().return_buses();
        let count = tracks.len() + returns.len();
        if count == 0 {
            return;
        }
        let index = self.cursor % count;
        self.cursor = (index + 1) % count;
        let (owner, meter) = if index < tracks.len() {
            (Owner::Track(index), &tracks[index].instrument_meter)
        } else {
            let index = index - tracks.len();
            (Owner::Return(index), &returns[index].output_meter)
        };
        self.target = meter
            .render_work()
            .requested_epoch()
            .map(|epoch| (owner, epoch));
    }

    pub(super) fn track(&self, index: usize) -> Option<Instant> {
        self.start(Owner::Track(index))
    }
    pub(super) fn return_bus(&self, index: usize) -> Option<Instant> {
        self.start(Owner::Return(index))
    }
    fn start(&self, owner: Owner) -> Option<Instant> {
        (self.target.map(|target| target.0) == Some(owner)).then(Instant::now)
    }
    pub(super) fn finish(&mut self, start: Option<Instant>) {
        if let Some(start) = start {
            let elapsed = u64::try_from(start.elapsed().as_nanos()).unwrap_or(u64::MAX);
            self.elapsed_ns = self.elapsed_ns.saturating_add(elapsed);
            self.scopes = self.scopes.saturating_add(1);
        }
    }
    pub(super) fn publish(&self, revision: &PreparedRevision, frames: usize, valid: bool) {
        if !valid || self.scopes == 0 {
            return;
        }
        if let Some((owner, epoch)) = self.target {
            let meter = match owner {
                Owner::Track(index) => revision
                    .arrangement()
                    .tracks()
                    .get(index)
                    .map(|track| &track.instrument_meter),
                Owner::Return(index) => revision
                    .arrangement()
                    .return_buses()
                    .get(index)
                    .map(|bus| &bus.output_meter),
            };
            if let Some(meter) = meter {
                meter.render_work().publish(
                    epoch,
                    self.elapsed_ns,
                    frames,
                    revision.arrangement().sample_rate(),
                );
            }
        }
    }
}
