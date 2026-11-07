//! Delivery-confirmed native device input to the existing durable draft lane.
//! The caller retains the original receipts until the whole batch is admitted.
//! Scope and terminal come from the exact audio endpoint. This control bridge
//! owns neither endpoint admission nor the canonical adoption of its draft.
use super::{
    AppliedEvent, DraftInput, DraftLaneProgress, DraftProvenance, DraftSubmitError, DraftTarget,
    DraftTerminal, DraftWorker, DraftWorkerOutcome, DraftWriteAck, DraftWriter, Session, recovery,
    spawn_writer_on_control,
};
use hex_engine::{NativeParameterApplication, NativeParameterCapture, NativeParameterInputTarget};
use hex_storage::automation_draft::{
    CaptureClock, DraftAbortReason, DraftClock, MAX_APPEND_EVENTS,
};
use std::path::{Path, PathBuf};

pub struct DeviceDraftInput {
    input: DraftInput,
    capture: NativeParameterCapture,
    target: NativeParameterInputTarget,
    stamp: DraftProvenance,
    last_sequence: u64,
}

impl DeviceDraftInput {
    /// Capture clock and provenance are from the live output that owns the
    /// armed gesture. Creating a draft confers no native input authority.
    pub fn start_on_control(
        session: &Session,
        document_base: Option<&Path>,
        path: PathBuf,
        target: NativeParameterInputTarget,
        stamp: DraftProvenance,
        clock: CaptureClock,
    ) -> Result<(Self, DraftWorker), String> {
        let capture = NativeParameterCapture::prepare(session.project(), target)
            .map_err(|error| error.0.to_owned())?;
        let source = recovery::project_for_save(session, document_base)?;
        let path = recovery::absolute_document_path(&path)?;
        let writer = DraftWriter::create_with_clock(
            path,
            &source,
            DraftTarget::Device {
                device: target.device(),
                target: target.target(),
            },
            stamp,
            clock,
        )
        .map_err(|error| error.to_string())?;
        let (input, worker) = spawn_writer_on_control(writer)?;
        Ok((
            Self {
                input,
                capture,
                target,
                stamp,
                last_sequence: 0,
            },
            worker,
        ))
    }

    /// Borrow only actual Applied receipts. Superseded requests are omitted
    /// and may leave sequence gaps. None is bounded writer backpressure and
    /// leaves the entire receipt batch and sequence watermark with the caller.
    pub fn append_delivered(
        &mut self,
        target: NativeParameterInputTarget,
        stamp: DraftProvenance,
        applications: &[(u64, NativeParameterApplication)],
    ) -> Result<Option<u64>, &'static str> {
        self.require_scope(target, stamp)?;
        if !(1..=MAX_APPEND_EVENTS).contains(&applications.len()) {
            return Err("native capture batch size is outside its bound");
        }
        let Some(mut batch) = self.input.reserve_batch()? else {
            return Ok(None);
        };
        let mut last = self.last_sequence;
        for &(sequence, application) in applications {
            if sequence <= last {
                return Err("native capture sequence did not advance");
            }
            let value = self
                .capture
                .value(application)
                .map_err(|error| error.0)?
                .ok_or("native capture application restored the lane")?;
            batch
                .push(AppliedEvent {
                    sequence,
                    performance_frame: application.performance_frame,
                    content_frame: application.arrangement_frame,
                    loop_pass: application.loop_pass,
                    value,
                })
                .map_err(|_| "native capture batch size is outside its bound")?;
            last = sequence;
        }
        let ticket = batch.submit().map_err(|error| match error {
            DraftSubmitError::Refused(batch) => batch.reason(),
            DraftSubmitError::InvalidReservation => "native capture reservation is unavailable",
        })?;
        self.last_sequence = last;
        Ok(Some(ticket))
    }

    /// A delivered Clear is the exclusive end of the touched range.
    pub fn finish_delivered(
        &mut self,
        target: NativeParameterInputTarget,
        stamp: DraftProvenance,
        restoration: NativeParameterApplication,
    ) -> Result<(), &'static str> {
        self.require_scope(target, stamp)?;
        if self
            .capture
            .value(restoration)
            .map_err(|error| error.0)?
            .is_some()
        {
            return Err("native capture terminal did not restore the lane");
        }
        self.input.finish(DraftTerminal::Finished(DraftClock {
            performance_frame: restoration.performance_frame,
            content_frame: restoration.arrangement_frame,
            loop_pass: restoration.loop_pass,
        }))
    }

    pub fn abort_delivered(
        &mut self,
        target: NativeParameterInputTarget,
        stamp: DraftProvenance,
        reason: DraftAbortReason,
    ) -> Result<(), &'static str> {
        self.require_scope(target, stamp)?;
        self.input.finish(DraftTerminal::Aborted(reason))
    }

    pub fn poll(&mut self) -> Option<DraftWriteAck> {
        self.input.poll()
    }

    pub fn progress(&self) -> DraftLaneProgress {
        self.input.progress()
    }

    pub fn try_take_outcome(&mut self) -> Option<DraftWorkerOutcome> {
        self.input.try_take_outcome()
    }

    fn require_scope(
        &self,
        target: NativeParameterInputTarget,
        stamp: DraftProvenance,
    ) -> Result<(), &'static str> {
        if self.target != target || self.stamp != stamp {
            return Err("native capture receipt scope differs from its draft");
        }
        Ok(())
    }
}
