//! In-memory ownership of one active and the latest pending directory scan.
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ScanRequest {
    pub(super) generation: u64,
    pub(super) directory: PathBuf,
}

#[derive(Default)]
pub(super) struct ScanCoordinator {
    pub(super) generation: u64,
    pub(super) in_flight: bool,
    pub(super) pending: Option<ScanRequest>,
}

impl ScanCoordinator {
    pub(super) fn request(&mut self, directory: PathBuf) -> Option<ScanRequest> {
        self.generation = self.generation.saturating_add(1);
        let request = ScanRequest {
            generation: self.generation,
            directory,
        };
        if self.in_flight {
            self.pending = Some(request);
            None
        } else {
            self.in_flight = true;
            Some(request)
        }
    }

    pub(super) fn complete(&mut self, generation: u64) -> (bool, Option<ScanRequest>) {
        self.in_flight = false;
        let current = generation == self.generation;
        let next = self.pending.take();
        if next.is_some() {
            self.in_flight = true;
        }
        (current, next)
    }
}

#[cfg(all(test, feature = "ui-unit-tier"))]
mod tests {
    use super::*;

    #[test]
    fn stale_directory_scan_results_are_not_current() {
        let mut scans = ScanCoordinator::default();
        let first = scans
            .request(PathBuf::from("/one"))
            .expect("first scan starts");
        assert!(scans.request(PathBuf::from("/two")).is_none());
        assert!(scans.request(PathBuf::from("/three")).is_none());
        let (first_is_current, next) = scans.complete(first.generation);
        assert!(!first_is_current);
        assert_eq!(
            next.expect("latest scan starts next").directory,
            PathBuf::from("/three")
        );
        assert!(scans.in_flight);
        assert!(scans.pending.is_none());
    }
}
