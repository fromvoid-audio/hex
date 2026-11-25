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
        if let Some(Processor::Native(instance)) = self.processor.as_mut() {
            while instance.has_pending_control_capture_on_control()
                || instance.has_pending_control_controller_on_control()
            {
                if instance.has_pending_control_capture_on_control() {
                    match instance.poll_state_capture_on_worker() {
                        Ok(Some(_)) | Ok(None) | Err(Error::CaptureRefused(_)) => {}
                        Err(error) => return Err(error),
                    }
                }
                if instance.has_pending_control_controller_on_control() {
                    let _ = instance.poll_parameter_page_on_worker()?;
                }
                if instance.has_pending_control_capture_on_control()
                    || instance.has_pending_control_controller_on_control()
                {
                    thread::sleep(Duration::from_millis(1));
                }
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
        self.restore_control_client_into_joined_state();
        let state = self
            .reaper
            .worker
            .as_mut()
            .ok_or("native stopped capture owner is missing")?;
        if state.native_error.is_some() || !matches!(state.processor, Some(Processor::Native(_))) {
            return Err("native stopped capture source failed");
        }
        if !state.capture.is_in_flight()
            && let Some(Processor::Native(instance)) = state.processor.as_mut()
            && instance.has_pending_control_capture_on_control()
        {
            match instance.poll_state_capture_on_worker() {
                Ok(None) => return Err("native preparation is busy"),
                Ok(Some(_)) | Err(Error::CaptureRefused(_)) => {
                    if let Ok(mut slot) = self.reaper.completed.capture.try_lock()
                        && slot.mode == Mode::Live
                        && slot.via_control_channel
                    {
                        slot.phase = Phase::Idle;
                        slot.via_control_channel = false;
                    }
                }
                Err(error) => {
                    state.native_error = Some(error);
                    return Err("native stopped capture failed");
                }
            }
        }
        // The stopped capture now drives the original transport's control
        // lane. Finish an already admitted edit read before capturing, while
        // keeping its result in the same mailbox for the original reader.
        // A turn still performs at most one bounded SDK exchange.
        let signal = &self.reaper.completed;
        let progress = if signal.edits_requested.load(Acquire) {
            state.advance_parameter_edits(signal)
        } else {
            state.advance_capture(signal)
        };
        if let Err(error) = progress {
            state.native_error = Some(error);
            return Err("native stopped capture failed");
        }
        Ok(())
    }
}
