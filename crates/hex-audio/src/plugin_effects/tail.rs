//! Original endpoint's accepted zero-input prefix. SDK reports are residual
//! frames; the copied bridge and processor latency must drain as well. Pending
//! zeros cannot extend the committed prefix, and new input revokes a previously
//! drained view immediately. No scalar here grants graph/source retirement.
pub(super) struct TailDrain {
    horizon: Option<u64>,
    committed_zeros: u64,
    pending_zeros: u64,
}
impl TailDrain {
    pub(super) fn new(reported_frames: u32, total_latency: u32) -> Self {
        Self {
            horizon: match reported_frames {
                u32::MAX => None,
                frames => u64::from(frames).checked_add(u64::from(total_latency)),
            },
            committed_zeros: 0,
            pending_zeros: 0,
        }
    }
    pub(super) fn begin(&mut self) {
        self.pending_zeros = self.committed_zeros;
    }
    pub(super) fn input(&mut self, frames: &[[f32; 2]]) {
        let Some(horizon) = self.horizon else {
            return;
        };
        for frame in frames {
            self.pending_zeros = if frame.iter().all(|sample| *sample == 0.0) {
                self.pending_zeros.saturating_add(1).min(horizon)
            } else {
                0
            };
        }
    }
    pub(super) fn commit(&mut self) {
        self.committed_zeros = self.pending_zeros;
    }
    pub(super) fn drained(&self) -> bool {
        self.horizon
            .is_some_and(|horizon| self.committed_zeros >= horizon && self.pending_zeros >= horizon)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reported_residual_and_fixed_latency_need_accepted_zeros_across_partitions() {
        let mut tail = TailDrain::new(3, 16);
        tail.begin();
        tail.input(&[[0.5, 0.0]]);
        tail.input(&[[0.0; 2]; 7]);
        tail.commit();
        tail.begin();
        tail.input(&[[0.0; 2]; 8]);
        tail.commit();
        tail.begin();
        tail.input(&[[0.0; 2]; 4]);
        assert!(
            !tail.drained(),
            "nineteen speculative zeros are not acceptance"
        );
        tail.commit();
        assert!(tail.drained());
        tail.begin();
        tail.input(&[[0.0; 2]; 2]);
        assert!(tail.drained());
        tail.input(&[[0.0, -0.25]]);
        assert!(
            !tail.drained(),
            "new input invalidates the drained view before commit"
        );
        tail.commit();
        assert!(!tail.drained());
    }

    #[test]
    fn infinite_report_never_becomes_a_finite_drain_and_zero_report_keeps_bridge() {
        let mut infinite = TailDrain::new(u32::MAX, 16);
        infinite.begin();
        infinite.input(&[[0.0; 2]; 256]);
        infinite.commit();
        assert!(!infinite.drained());
        let mut zero = TailDrain::new(0, 16);
        zero.begin();
        zero.input(&[[0.0; 2]; 15]);
        zero.commit();
        assert!(!zero.drained());
        zero.begin();
        zero.input(&[[0.0; 2]]);
        assert!(!zero.drained());
        zero.commit();
        assert!(zero.drained());
    }
}
