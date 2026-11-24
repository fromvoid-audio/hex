//! Reports which installed vst3 devices are degraded (#4569, #13237). After #3977 a
//! faulted endpoint stays in the callback bank as an aligned pass-through while
//! the other plugins keep processing. Its transport is no longer live. This
//! pure control state turns per-binding liveness and actual failure diagnostics,
//! already polled on the Session worker, into one change notice per output and
//! keeps per-device fault isolation across unrelated edits and Stop/Restore
//! until the device is explicitly reloaded, removed, powered off, or the output
//! changes. The audio callback does no work for it.

pub(super) const DEFAULT_DEGRADED_REASON: &str = "vst3 stopped responding";

/// Normalize a raw native worker or callback diagnostic into a concise, bounded
/// lowercase failure reason suitable for UI status/tooltips.
pub(super) fn summarize_degraded_reason(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return DEFAULT_DEGRADED_REASON.to_owned();
    }
    let lowered = trimmed.to_lowercase();
    let concise = if let Some((_, after_cause)) = lowered.split_once("cause=") {
        after_cause
            .split(" reply_sequence=")
            .next()
            .unwrap_or(after_cause)
            .trim()
    } else if let Some((_, after_colon)) = lowered.split_once("vst3 callback transport failed:") {
        after_colon.trim()
    } else {
        lowered.as_str()
    };
    if concise.is_empty() {
        DEFAULT_DEGRADED_REASON.to_owned()
    } else {
        concise.chars().take(160).collect()
    }
}

/// Last known degradation per source binding, plus the sorted `(device, reason)`
/// set last sent for one output. A set is reported only when it changes, and a
/// new output always reports its first set.
#[derive(Debug)]
pub(super) struct DegradedReport<K> {
    output: Option<u64>,
    known: Vec<(K, u64, bool)>,
    devices: Vec<(u64, String)>,
    reported: Option<Vec<(u64, String)>>,
}
impl<K> Default for DegradedReport<K> {
    fn default() -> Self {
        Self {
            output: None,
            known: Vec::new(),
            devices: Vec::new(),
            reported: None,
        }
    }
}
impl<K: Copy + PartialEq> DegradedReport<K> {
    fn reset_output(&mut self, output: u64) {
        if self.output != Some(output) {
            self.output = Some(output);
            self.known.clear();
            self.devices.clear();
            self.reported = None;
        }
    }

    fn record_device_reason(&mut self, device: u64, diagnostic: Option<&str>) {
        let summarized = diagnostic.map(summarize_degraded_reason);
        if let Some((_, existing)) = self.devices.iter_mut().find(|(id, _)| *id == device) {
            if let Some(reason) = summarized
                && (existing == DEFAULT_DEGRADED_REASON || reason != DEFAULT_DEGRADED_REASON)
            {
                *existing = reason;
            }
        } else {
            self.devices.push((
                device,
                summarized.unwrap_or_else(|| DEFAULT_DEGRADED_REASON.to_owned()),
            ));
        }
    }

    fn commit_change(&mut self) -> Option<Vec<(u64, String)>> {
        self.devices.sort_by_key(|(device, _)| *device);
        self.devices.dedup_by_key(|(device, _)| *device);
        if self.reported.as_ref() == Some(&self.devices) {
            return None;
        }
        self.reported = Some(self.devices.clone());
        Some(self.devices.clone())
    }

    /// Whether `binding` on `output` was observed degraded.
    pub(super) fn is_binding_degraded(&self, output: u64, binding: K) -> bool {
        self.output == Some(output)
            && self
                .known
                .iter()
                .any(|(known, _, degraded)| *known == binding && *degraded)
    }

    /// Whether `device` on `output` is currently isolated as degraded.
    pub(super) fn is_device_degraded(&self, output: u64, device: u64) -> bool {
        self.output == Some(output) && self.devices.iter().any(|(id, _)| *id == device)
    }

    /// Explicitly isolate a single binding/device after a control-side fault
    /// (for example during stopped epoch renewal) without disturbing other
    /// healthy plugins on `output`.
    pub(super) fn mark_degraded(
        &mut self,
        output: u64,
        binding: K,
        device: u64,
        diagnostic: Option<&str>,
    ) -> Option<Vec<(u64, String)>> {
        self.reset_output(output);
        if let Some(entry) = self
            .known
            .iter_mut()
            .find(|(known, _, _)| *known == binding)
        {
            entry.2 = true;
        } else {
            self.known.push((binding, device, true));
        }
        self.record_device_reason(device, diagnostic);
        self.commit_change()
    }

    /// Clear a device's degraded state after a fresh live instance is accepted.
    pub(super) fn clear_device(&mut self, output: u64, device: u64) -> Option<Vec<(u64, String)>> {
        if self.output != Some(output) {
            return None;
        }
        self.known.retain(|(_, id, _)| *id != device);
        self.devices.retain(|(id, _)| *id != device);
        self.commit_change()
    }

    /// Updates each present source from its `(loaned, live)` status and optional
    /// failure diagnostic, returning the sorted `(device, reason)` set when it
    /// differs from the last one reported for `output`.
    ///
    /// A loaned endpoint whose transport is no longer live is degraded. An
    /// unavailable status (contention, retiring) keeps the binding's last known
    /// state, so a busy poll does not flicker. Devices already isolated as
    /// degraded remain tracked while still present and active in
    /// `active_project_devices`, even after their dead source slot is retired
    /// during an unrelated edit or Stop/Restore, and clear as soon as a live
    /// replacement endpoint is observed or the device is removed/powered off.
    pub(super) fn observe(
        &mut self,
        output: u64,
        endpoints: impl IntoIterator<Item = (K, u64, Option<(bool, bool)>, Option<String>)>,
        active_project_devices: Option<&[u64]>,
    ) -> Option<Vec<(u64, String)>> {
        self.reset_output(output);
        let previous = std::mem::take(&mut self.known);
        let mut live_devices = Vec::new();
        for (binding, device, status, diagnostic) in endpoints {
            let degraded = match status {
                Some((true, false)) => {
                    self.record_device_reason(device, diagnostic.as_deref());
                    true
                }
                Some((true, true)) => {
                    live_devices.push(device);
                    false
                }
                Some(_) => false,
                None => previous
                    .iter()
                    .any(|(known, _, degraded)| *known == binding && *degraded),
            };
            self.known.push((binding, device, degraded));
        }
        if !live_devices.is_empty() {
            self.devices
                .retain(|(device, _)| !live_devices.contains(device));
        }
        if let Some(active) = active_project_devices {
            self.devices.retain(|(device, _)| active.contains(device));
        }
        self.commit_change()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarize_degraded_reason_preserves_actual_cause_without_fabricating_deadline() {
        assert_eq!(
            summarize_degraded_reason(
                "vst3 callback transport failed: binding=EffectBinding { root: 1 } completed_blocks=3 fault=MissingOutput cause=vst3 worker missed audio deadline reply_sequence=4 worker_phase=Idle"
            ),
            "vst3 worker missed audio deadline"
        );
        assert_eq!(
            summarize_degraded_reason(
                "vst3 callback transport failed: binding=x fault=WorkerExited cause=VST3 Helper Process Exited"
            ),
            "vst3 helper process exited"
        );
        assert_eq!(
            summarize_degraded_reason("Host refused callback during process"),
            "host refused callback during process"
        );
        assert_eq!(summarize_degraded_reason("   "), DEFAULT_DEGRADED_REASON);
        assert!(!DEFAULT_DEGRADED_REASON.contains("deadline"));
    }

    #[test]
    fn only_loaned_endpoints_that_lost_their_transport_are_degraded() {
        let mut report = DegradedReport::default();
        let healthy = [
            (10, 1, Some((true, true)), None),
            (20, 2, Some((true, true)), None),
        ];
        assert_eq!(
            report.observe(7, healthy.clone(), Some(&[1, 2])),
            Some(vec![])
        );
        assert_eq!(
            report.observe(7, healthy, Some(&[1, 2])),
            None,
            "unchanged sets stay quiet"
        );
        let faulted = [
            (
                20,
                2,
                Some((true, false)),
                Some("vst3 helper exited".to_owned()),
            ),
            (10, 1, Some((true, true)), None),
            // A returned or warm source outside the callback bank is not.
            (30, 3, Some((false, false)), None),
            (40, 4, None, None),
        ];
        assert_eq!(
            report.observe(7, faulted.clone(), Some(&[1, 2, 3, 4])),
            Some(vec![(2, "vst3 helper exited".to_owned())])
        );
        assert!(report.is_binding_degraded(7, 20));
        assert!(report.is_device_degraded(7, 2));
        assert!(!report.is_device_degraded(7, 1));
        assert_eq!(report.observe(7, faulted, Some(&[1, 2, 3, 4])), None);
    }

    #[test]
    fn degraded_device_survives_retired_source_until_reloaded_or_removed() {
        let mut report = DegradedReport::default();
        assert_eq!(
            report.observe(
                7,
                [(
                    20,
                    2,
                    Some((true, false)),
                    Some("vst3 process refused".to_owned()),
                )],
                Some(&[1, 2]),
            ),
            Some(vec![(2, "vst3 process refused".to_owned())])
        );
        // Busy poll keeps state with no flicker.
        assert_eq!(
            report.observe(7, [(20, 2, None, None)], Some(&[1, 2])),
            None
        );
        // After an unrelated edit or Stop retires dead binding 20 while device 2
        // remains in the project, device 2 stays degraded with its reason.
        assert_eq!(
            report.observe(7, [(10, 1, Some((true, true)), None)], Some(&[1, 2])),
            None
        );
        assert!(report.is_device_degraded(7, 2));
        // A single-plugin reload installs live binding 21 for device 2 -> cleared!
        assert_eq!(
            report.observe(
                7,
                [
                    (10, 1, Some((true, true)), None),
                    (21, 2, Some((true, true)), None),
                ],
                Some(&[1, 2]),
            ),
            Some(vec![])
        );
        assert!(!report.is_device_degraded(7, 2));

        // Removing/powering off a degraded device also clears it cleanly.
        assert_eq!(
            report.mark_degraded(7, 21, 2, Some("quiesce refused")),
            Some(vec![(2, "quiesce refused".to_owned())])
        );
        assert_eq!(
            report.observe(7, [(10, 1, Some((true, true)), None)], Some(&[1])),
            Some(vec![])
        );
    }

    #[test]
    fn a_new_output_reports_its_own_set_again() {
        let mut report = DegradedReport::default();
        assert_eq!(
            report.observe(7, [(20, 2, Some((true, false)), None)], None),
            Some(vec![(2, DEFAULT_DEGRADED_REASON.to_owned())])
        );
        assert_eq!(report.observe(8, [(20, 2, None, None)], None), Some(vec![]));
        assert_eq!(
            report.observe(
                8,
                [(20, 2, Some((true, false)), Some("deadline".to_owned()))],
                None
            ),
            Some(vec![(2, "deadline".to_owned())])
        );
    }
}
