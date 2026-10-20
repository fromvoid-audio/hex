//! Concrete native Editor transport profile, not a DATA/native certificate.
//! The shared candidate bound preserves the existing UI admission behavior.
use super::*;

pub const NATIVE_PLAYBACK_CANDIDATE_CAPACITY: usize = 32;
const VIEW_ROLES_PER_EVENT: usize = 3;
const QUEUED_EVENTS: usize = super::source_queue::CAPACITY;
// Submitted admits only Q<32; resident ACK moves R into Q. Hence Q+R<=33,
// including the already-admitted resident's ACK after Submitted fills Q=32.
const QUEUED_PLUS_RESIDENT: usize = NATIVE_PLAYBACK_CANDIDATE_CAPACITY + 1;
const HELD_UI_READS: usize = 1 + 1 + 1 + QUEUED_PLUS_RESIDENT; // current/pending/audible
const WORKER_CURRENT_SNAPSHOTS: usize = 1;
const PRODUCER_READS: usize = VIEW_ROLES_PER_EVENT;
const RECEIVER_HEAD_SNAPSHOTS: usize = VIEW_ROLES_PER_EVENT;
// Three actual head roles plus one actual late role/derive. ACKs from this
// bounded transition may remain occupied until the next control collection.
// Claim collects ALL prior ACKs before reserving its extra canonical read.
const RECEIVER_TRANSITION_READS: usize = VIEW_ROLES_PER_EVENT + 1;

impl SessionExportFamily {
    /// Finite component quote BEFORE either family Vec birth. Actual snapshot
    /// metadata/Project/native/old-new children, outer shared/job body, guard
    /// scratch and BOTH whole-operation caps remain mandatory separately.
    /// This neither approves already-born storage nor changes the hard budget.
    pub fn native_editor_reservation_on_control(
        max_bookkeeping_bytes: usize,
    ) -> Result<SnapshotExportReservation, ExportRecordError> {
        let queued = QUEUED_EVENTS
            .checked_mul(VIEW_ROLES_PER_EVENT)
            .ok_or(ExportRecordError::Reservation)?;
        let snapshots = HELD_UI_READS
            .checked_add(WORKER_CURRENT_SNAPSHOTS)
            .and_then(|n| n.checked_add(queued))
            .and_then(|n| n.checked_add(PRODUCER_READS))
            .and_then(|n| n.checked_add(RECEIVER_HEAD_SNAPSHOTS))
            .ok_or(ExportRecordError::Reservation)?;
        let reads = HELD_UI_READS
            .checked_add(queued)
            .and_then(|n| n.checked_add(PRODUCER_READS))
            .and_then(|n| n.checked_add(RECEIVER_TRANSITION_READS))
            .ok_or(ExportRecordError::Reservation)?;
        let limits = SnapshotExportReservation {
            snapshot_records: snapshots,
            read_records: reads,
            max_bookkeeping_bytes,
        };
        let record_bytes = SnapshotExportRecords::reservation_quote(limits)?;
        // Exact changed inline queue/native pair/retirement/alignment. Never
        // price an old layout or charge the embedded record body twice.
        let family_overhead = std::mem::size_of::<Self>()
            .checked_sub(std::mem::size_of::<SnapshotExportRecords>())
            .ok_or(ExportRecordError::Reservation)?;
        let total = record_bytes
            .checked_add(family_overhead)
            .and_then(|n| n.checked_add(native_requests::NativeRequests::requested_backing().ok()?))
            .ok_or(ExportRecordError::Reservation)?;
        if total > max_bookkeeping_bytes {
            return Err(ExportRecordError::Reservation);
        }
        Ok(limits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn real_native_editor_tables_and_requests_fit_exact_budget_and_refuse_minus_one() {
        let requested = SessionExportFamily::native_editor_reservation_on_control(65_536).unwrap();
        assert_eq!(HELD_UI_READS, 36);
        let table = SnapshotExportRecords::reservation_quote(requested).unwrap();
        let exact = table + std::mem::size_of::<SessionExportFamily>()
            - std::mem::size_of::<SnapshotExportRecords>()
            + native_requests::NativeRequests::requested_backing().unwrap();
        let actual_limits =
            SessionExportFamily::native_editor_reservation_on_control(exact).unwrap();
        assert!(matches!(
            SessionExportFamily::native_editor_reservation_on_control(exact - 1),
            Err(ExportRecordError::Reservation)
        ));
        let (family, _) = SessionExportFamily::try_new_native_on_control(actual_limits).unwrap();
        family.prepare_requests_on_control().unwrap();
        println!(
            "COMPONENT_NATIVE_EDITOR_PROFILE snapshots={} reads={} queued_events={} candidate_bound={} ui_live_bound={} exact_family_tables_requests={} unchanged_hard_budget=65536; NO_PAYLOAD_NATIVE_OR_WHOLE_CAP_CERTIFICATE",
            actual_limits.snapshot_records,
            actual_limits.read_records,
            QUEUED_EVENTS,
            NATIVE_PLAYBACK_CANDIDATE_CAPACITY,
            HELD_UI_READS,
            exact
        );
    }
}
