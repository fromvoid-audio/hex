//! Capacity-one immutable export completion in the EXISTING Session family.
//! No Playback/runtime/compiled owner, lock, queue backing or registry here.
use super::{ExportRecordError as Error, *};
use crate::service::ExportEvent;
use std::sync::atomic::AtomicU8;

const READY: u8 = 0;
const READING: u8 = 1;
const ACKNOWLEDGED: u8 = 2;
const FAILED: u8 = 3;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Vacant,
    Queued,
    Retiring,
}

pub(super) struct NativeExportResult {
    enabled: bool,
    generation: u64,
    phase: Phase,
    payload: Option<ExportEvent>,
    read: AtomicU8,
}
impl Default for NativeExportResult {
    fn default() -> Self {
        Self {
            enabled: false,
            generation: 0,
            phase: Phase::Vacant,
            payload: None,
            read: AtomicU8::new(READY),
        }
    }
}
impl NativeExportResult {
    #[cfg(test)]
    pub(super) fn test_retains_actual_payload(&self) -> bool {
        self.payload.is_some()
    }
    pub(super) fn prepare(&mut self) -> Result<(), Error> {
        self.validate()?;
        self.enabled = true;
        Ok(())
    }
    fn validate(&self) -> Result<(), Error> {
        if self.read.load(Ordering::Acquire) == FAILED
            || (self.phase == Phase::Vacant && self.payload.is_some())
            || (self.phase == Phase::Queued && self.payload.is_none())
        {
            return Err(Error::Stale);
        }
        Ok(())
    }
    pub(super) fn send(&mut self, source: &mut Option<ExportEvent>) -> Result<(), Error> {
        self.validate()?;
        if !self.enabled {
            return Err(Error::Closed);
        }
        if self.phase != Phase::Vacant {
            return Err(Error::Reservation);
        }
        if source.is_none() {
            return Err(Error::Stale);
        }
        let generation = self.generation.checked_add(1).ok_or(Error::Exhausted)?;
        // Fixed take/state publication only after all refusals. No Drop/clone.
        self.payload = source.take();
        self.generation = generation;
        self.read.store(READY, Ordering::Relaxed);
        self.phase = Phase::Queued;
        Ok(())
    }
    pub(super) fn read<R>(
        &self,
        read: impl FnOnce(ExportResultRead<'_>) -> R,
    ) -> Result<Option<R>, Error> {
        self.validate()?;
        if !self.enabled {
            return Err(Error::Closed);
        }
        if self.phase != Phase::Queued {
            return Ok(None);
        }
        let payload = self.payload.as_ref().ok_or(Error::Stale)?;
        match self
            .read
            .compare_exchange(READY, READING, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) => {}
            Err(READING | ACKNOWLEDGED) => return Ok(None),
            Err(_) => return Err(Error::Stale),
        }
        struct ReadAttempt<'a> {
            state: &'a AtomicU8,
            finished: bool,
        }
        impl Drop for ReadAttempt<'_> {
            fn drop(&mut self) {
                self.state.store(
                    if self.finished { ACKNOWLEDGED } else { FAILED },
                    Ordering::Release,
                );
            }
        }
        let mut attempt = ReadAttempt {
            state: &self.read,
            finished: false,
        };
        let result = read(ExportResultRead { event: payload });
        attempt.finished = true;
        drop(attempt); // Durable ACK only after the actual borrowed read returns.
        Ok(Some(result))
    }
    pub(super) fn fields(
        &self,
    ) -> Result<
        hex_engine::sampler_host_fields::FamilyExportResultFields<'_>,
        hex_engine::AudioStorageError,
    > {
        self.validate()
            .map_err(|_| hex_engine::AudioStorageError::UnknownStorage)?;
        if self.phase == Phase::Retiring {
            return Err(hex_engine::AudioStorageError::UnknownStorage);
        }
        Ok(super::storage::export_result_fields(
            self,
            self.payload.as_ref(),
        ))
    }
    pub(super) fn begin_cleanup(
        &mut self,
        family: u64,
        abandoned: bool,
        destination: &mut ExportResultRetirement,
    ) -> Result<bool, Error> {
        self.validate()?;
        if !destination.vacant() || self.phase == Phase::Retiring {
            return Err(Error::Stale);
        }
        if self.phase != Phase::Queued {
            return Ok(false);
        }
        let state = self.read.load(Ordering::Acquire);
        if state == READING || (!abandoned && state != ACKNOWLEDGED) {
            return Ok(false);
        }
        if state == FAILED {
            return Err(Error::Stale);
        }
        destination.family = family;
        destination.generation = self.generation;
        destination.pending = true; // BEFORE take; persistent original destination.
        self.phase = Phase::Retiring;
        destination.payload = self.payload.take();
        Ok(true)
    }
    pub(super) fn finish_cleanup(
        &mut self,
        family: u64,
        destination: &mut ExportResultRetirement,
    ) -> Result<(), Error> {
        if self.phase != Phase::Retiring
            || self.payload.is_some()
            || destination.family != family
            || destination.generation != self.generation
            || !destination.pending
            || !destination.released
            || destination.payload.is_some()
        {
            return Err(Error::Stale);
        }
        self.phase = Phase::Vacant;
        self.read.store(READY, Ordering::Relaxed);
        destination.pending = false;
        destination.released = false;
        Ok(())
    }
    pub(super) fn vacant(&self) -> bool {
        self.phase == Phase::Vacant && self.payload.is_none()
    }
}

/// Actual existing-family disposal destination. A consumed/panicking Drop is
/// never retried or considered vacancy. Outer custodian retains first failure.
#[derive(Default)]
pub(super) struct ExportResultRetirement {
    family: u64,
    generation: u64,
    payload: Option<ExportEvent>,
    pending: bool,
    released: bool,
}
impl ExportResultRetirement {
    pub(super) fn vacant(&self) -> bool {
        !self.pending && self.payload.is_none()
    }
    pub(super) fn release(&mut self) {
        // Already pending; this take/drop happens with NO inner family guard.
        drop(self.payload.take());
        self.released = true;
    }
    pub(super) fn sampler_fields(
        &self,
    ) -> Result<
        hex_engine::sampler_host_fields::FamilyExportResultFields<'_>,
        hex_engine::AudioStorageError,
    > {
        let Self {
            family: _,
            generation: _,
            payload,
            pending,
            released: _,
        } = self;
        if *pending && payload.is_none() {
            return Err(hex_engine::AudioStorageError::UnknownStorage);
        }
        Ok(super::storage::export_result_fields(self, payload.as_ref()))
    }
    pub(super) fn visit<'a>(
        &'a self,
        ledger: &mut hex_engine::AudioRootStorageLedger<'a>,
    ) -> Result<(), hex_engine::AudioStorageError> {
        if self.pending && self.payload.is_none() {
            return Err(hex_engine::AudioStorageError::UnknownStorage);
        }
        ledger.borrow_sampler_export_result_fields_on_control(super::storage::export_result_fields(
            self,
            self.payload.as_ref(),
        ))
    }
}

/// Only immutable metadata queries; no renderer/graph/Project owner getter.
#[derive(Clone, Copy)]
pub struct ExportResultRead<'a> {
    event: &'a ExportEvent,
}
impl<'a> ExportResultRead<'a> {
    pub(crate) fn of(event: &'a ExportEvent) -> Self {
        Self { event }
    }
    pub fn revision(self) -> u64 {
        self.event.revision
    }
    pub fn path(self) -> &'a std::path::Path {
        &self.event.path
    }
    pub fn result(self) -> Result<ExportReportRead<'a>, &'a str> {
        self.event
            .result
            .as_ref()
            .map(|report| ExportReportRead { report })
            .map_err(String::as_str)
    }
}
#[derive(Clone, Copy)]
pub struct ExportReportRead<'a> {
    report: &'a hex_export::ExportReport,
}
impl ExportReportRead<'_> {
    pub fn frames(self) -> u64 {
        self.report.frames
    }
    pub fn sample_rate(self) -> u32 {
        self.report.sample_rate
    }
    pub fn data_bytes(self) -> u32 {
        self.report.data_bytes
    }
    pub fn path(&self) -> &std::path::Path {
        &self.report.path
    }
    pub fn recovery_path(&self) -> Option<&std::path::Path> {
        self.report.recovery_path.as_deref()
    }
}

#[cfg(test)]
mod owner_tests {
    use super::*;
    #[test]
    fn native_result_owning_payload_cannot_be_certified_as_vacant() {
        let mut record = NativeExportResult::default();
        record.prepare().unwrap();
        record
            .send(&mut Some(ExportEvent {
                revision: 1,
                path: "actual.wav".into(),
                result: Err("actual owned error".into()),
            }))
            .unwrap();
        record.phase = Phase::Vacant;
        assert!(matches!(
            record.fields(),
            Err(hex_engine::AudioStorageError::UnknownStorage)
        ));
    }
}
