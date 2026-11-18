//! Data-only capture after the original transport thread's actual return. Its
//! retained State, processor, channel, rings and native lease remain in Reaper.
//! No callback/graph return, quiescent transfer or processing eligibility follows.
use super::*;

impl State {
    pub(in crate::plugin_worker) fn drain_capture_before_quiescence(
        &mut self,
        signal: &Signal,
    ) -> Result<(), Error> {
        // Joining has already returned this exact State. A live token may have
        // failed on revocation while its admitted native main-lane call is still
        // running. Preserve that instance by observing the call's real terminal
        // before setProcessing(false). Existing SDK deadlines/cancellation bound
        // this drain; the sleep is only the existing control polling cadence.
        while self.capture.is_in_flight() || self.controller.is_in_flight() {
            if self.capture.is_in_flight() {
                self.advance_capture(signal)?;
            } else {
                self.advance_controller(signal)?;
            }
            if self.capture.is_in_flight() || self.controller.is_in_flight() {
                thread::sleep(Duration::from_millis(1));
            }
        }
        Ok(())
    }
}

impl NativeWorker {
    pub(in crate::plugin_worker) fn prepare_stopped_capture(&mut self) -> Result<(), &'static str> {
        {
            let slot = self
                .reaper
                .completed
                .capture
                .try_lock()
                .map_err(|_| "native capture mailbox is busy")?;
            if slot.phase != Phase::Idle && slot.mode == Mode::Stopped {
                return Err("native capture is already pending");
            }
        }
        self.service_stopped_capture()?;
        let mut slot = self
            .reaper
            .completed
            .capture
            .try_lock()
            .map_err(|_| "native capture mailbox is busy")?;
        if slot.phase == Phase::Requested {
            return Err("native preparation is busy");
        }
        // A live token already reached Failed when its transport was revoked.
        // Drain its original native operation before discarding late data. Do
        // not strand the SDK request or reuse it as the new Save operation.
        if slot.phase == Phase::Complete && slot.mode == Mode::Live {
            let obsolete = slot.take_terminal();
            drop(slot);
            drop(obsolete);
        }
        Ok(())
    }

    pub(super) fn service_stopped_capture(&mut self) -> Result<(), &'static str> {
        if self.retirement_requested
            || self.transport_is_live_on_control()
            || !self.retains_original_source_on_control()
        {
            return Err("native stopped capture source differs");
        }
        if self
            .reaper
            .join
            .as_ref()
            .is_some_and(|join| !join.is_finished())
        {
            return Err("native preparation is busy");
        }
        // is_finished is only an observation. Join and take the original
        // terminal before borrowing State; a failed completion never qualifies.
        self.reaper
            .join_on_control()
            .map_err(|_| "native stopped capture worker failed")?;
        let state = self
            .reaper
            .worker
            .as_mut()
            .ok_or("native stopped capture owner is missing")?;
        if state.native_error.is_some() || !matches!(state.processor, Some(Processor::Native(_))) {
            return Err("native stopped capture source failed");
        }
        // At most the same one bounded native control exchange as the former
        // transport loop. Its existing poll cadence and SDK deadline still own
        // completion. No mailbox guard or bank spans that exchange.
        if let Err(error) = state.advance_capture(&self.reaper.completed) {
            state.native_error = Some(error);
            return Err("native stopped capture failed");
        }
        Ok(())
    }
}
