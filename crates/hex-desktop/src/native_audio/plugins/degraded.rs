//! Reports which installed vst3 devices are degraded (#4569). After #3977 a
//! faulted endpoint stays in the callback bank as an aligned pass-through while
//! the other plugins keep processing. Its transport is no longer live. This
//! pure control state turns the per-binding liveness, already polled on the
//! Session worker, into one change notice per output. The audio callback does
//! no work for it.

/// Last known degradation per source binding, plus the device set last sent
/// for one output. A set is reported only when it changes, and a new output
/// always reports its first set.
#[derive(Debug)]
pub(super) struct DegradedReport<K> {
    output: Option<u64>,
    known: Vec<(K, u64, bool)>,
    reported: Option<Vec<u64>>,
}
impl<K> Default for DegradedReport<K> {
    fn default() -> Self {
        Self {
            output: None,
            known: Vec::new(),
            reported: None,
        }
    }
}
impl<K: Copy + PartialEq> DegradedReport<K> {
    /// Updates each present source from its `(loaned, live)` status and
    /// returns the sorted degraded device set when it differs from the last
    /// one reported for `output`. A loaned endpoint whose transport is no
    /// longer live is degraded. An unavailable status (contention, retiring)
    /// keeps the binding's last known state, so a busy poll does not flicker.
    /// Sources missing from `endpoints` are forgotten.
    pub(super) fn observe(
        &mut self,
        output: u64,
        endpoints: impl IntoIterator<Item = (K, u64, Option<(bool, bool)>)>,
    ) -> Option<Vec<u64>> {
        if self.output != Some(output) {
            self.output = Some(output);
            self.known.clear();
            self.reported = None;
        }
        let previous = std::mem::take(&mut self.known);
        for (binding, device, status) in endpoints {
            let degraded = match status {
                Some(status) => status == (true, false),
                None => previous
                    .iter()
                    .any(|(known, _, degraded)| *known == binding && *degraded),
            };
            self.known.push((binding, device, degraded));
        }
        let mut devices: Vec<u64> = self
            .known
            .iter()
            .filter(|(_, _, degraded)| *degraded)
            .map(|(_, device, _)| *device)
            .collect();
        devices.sort_unstable();
        devices.dedup();
        if self.reported.as_ref() == Some(&devices) {
            return None;
        }
        self.reported = Some(devices.clone());
        Some(devices)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_loaned_endpoints_that_lost_their_transport_are_degraded() {
        let mut report = DegradedReport::default();
        let healthy = [(10, 1, Some((true, true))), (20, 2, Some((true, true)))];
        assert_eq!(report.observe(7, healthy), Some(vec![]));
        assert_eq!(
            report.observe(7, healthy),
            None,
            "unchanged sets stay quiet"
        );
        let faulted = [
            (20, 2, Some((true, false))),
            (10, 1, Some((true, true))),
            // A returned or warm source outside the callback bank is not.
            (30, 3, Some((false, false))),
            (40, 4, None),
        ];
        assert_eq!(report.observe(7, faulted), Some(vec![2]));
        assert_eq!(report.observe(7, faulted), None);
    }

    #[test]
    fn a_busy_status_keeps_the_last_state_until_the_source_is_replaced() {
        let mut report = DegradedReport::default();
        assert_eq!(
            report.observe(7, [(20, 2, Some((true, false)))]),
            Some(vec![2])
        );
        assert_eq!(report.observe(7, [(20, 2, None)]), None, "no flicker");
        // An edit replaced the endpoint: the old binding returned, the new
        // binding is live, and the device is healthy again.
        let replaced = [(20, 2, Some((false, false))), (21, 2, Some((true, true)))];
        assert_eq!(report.observe(7, replaced), Some(vec![]));
        assert_eq!(report.observe(7, [(21, 2, None)]), None);
    }

    #[test]
    fn a_new_output_reports_its_own_set_again() {
        let mut report = DegradedReport::default();
        assert_eq!(
            report.observe(7, [(20, 2, Some((true, false)))]),
            Some(vec![2])
        );
        assert_eq!(report.observe(8, [(20, 2, None)]), Some(vec![]));
        assert_eq!(
            report.observe(8, [(20, 2, Some((true, false)))]),
            Some(vec![2])
        );
    }
}
