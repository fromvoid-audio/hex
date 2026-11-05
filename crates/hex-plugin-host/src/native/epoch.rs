//! Same loaded child across an acknowledged, serialized processing discontinuity.
//! This does not move an Audio Root, callback endpoint or old queue. The audio
//! owner must quiesce/return those actual carriers before constructing successors.
use super::*;

/// Actual configured child with processing stopped and its completed PCM prefix
/// acknowledged. No PCM exchange API is exposed until a matching resume ACK.
/// Original startup capture is historical. Explicit capture returns fresh bytes;
/// resume itself never recaptures or restores state. Component/controller/source
/// custody stays in the same lease.
pub struct Quiescent {
    pub(super) instance: Prepared,
    facts: Facts,
    capture_sequence: u64,
    flush_sequence: u64,
    reactivate_sequence: u64,
}
impl Prepared {
    /// Check the actual child and cancellation state before transferring this
    /// unique lease to a worker. This is not graph admission or a future-liveness
    /// guarantee; subsequent exchange failures remain terminal.
    pub fn validate_worker_transfer_on_control(&mut self) -> Result<()> {
        self.require_live_epoch()
    }
    pub(super) fn require_live_epoch(&mut self) -> Result<()> {
        if self.faulted || self.retired.is_some() {
            return Err(Error::Protocol("instance lease is retired or faulted"));
        }
        self.lease.cancellation.check()?;
        if self.lease.child.observe()?.is_some() {
            return Err(Error::Custody("native epoch owner already exited".into()));
        }
        Ok(())
    }
    fn refuse_epoch_retained(self, cause: Error) -> PreparationFailure {
        PreparationFailure::native(cause, self.lease, self.limits.retirement)
    }
    /// Consume the unique serialized owner after its last completed exchange.
    /// Neither this I/O nor actual child retirement may run on callback.
    pub fn quiesce_on_control(self) -> Result<Quiescent> {
        self.quiesce_retained_on_control()
            .map_err(PreparationFailure::finish_on_control)
    }
    /// Failed discontinuity keeps the exact cancelled lease in existing failure
    /// custody. The caller must observe disposal independently before destroying
    /// that owner; neither the error nor Drop acknowledges Root/callback return.
    pub fn quiesce_retained_on_control(
        mut self,
    ) -> std::result::Result<Quiescent, PreparationFailure> {
        let result = (|| {
            self.require_live_epoch()?;
            if self.live_capture.is_some() {
                return Err(Error::Protocol("native capture is still pending"));
            }
            if self.parameter_page.is_some() {
                return Err(Error::Protocol("controller page is still pending"));
            }
            let until = end_after(self.limits.startup)?;
            let token = self.lease.cancellation.clone();
            let request = protocol::quiesce(&self.config, self.sequence)?;
            self.lease
                .channel
                .send(&request, until, &mut || token.check())?;
            self.lease
                .channel
                .read(&mut self.packet, until, &mut || token.check())?;
            if self.packet.magic() == b"HXVIER01" {
                return Err(Error::Worker(protocol::error(
                    self.packet.body()?,
                    self.config.binding,
                )?));
            }
            if self.packet.magic() != b"HXVIQF02" {
                return Err(Error::Protocol("missing actual quiescent acknowledgement"));
            }
            let facts =
                protocol::quiescent_facts(self.packet.body()?, &self.config, self.sequence)?;
            self.require_live_epoch()?;
            Ok(facts)
        })();
        match result {
            Ok(facts) => Ok(Quiescent {
                instance: self,
                facts,
                capture_sequence: 0,
                flush_sequence: 0,
                reactivate_sequence: 0,
            }),
            Err(cause) => Err(self.refuse_epoch_retained(cause)),
        }
    }
}
impl Quiescent {
    /// Settle the opted-in latency request before the owner computes its next
    /// compensation layout. Bound repeated SDK invalidations on control.
    pub fn settle_latency_retained_on_control(
        mut self,
    ) -> std::result::Result<Self, PreparationFailure> {
        for _ in 0..2 {
            match self.poll_recovery_edits_on_control() {
                Ok(batch) if batch.pending_latency_restart.is_none() => return Ok(self),
                Ok(_) => self = self.reactivate_retained_on_control()?,
                Err(error) => return Err(self.instance.refuse_epoch_retained(error)),
            }
        }
        let error = match self.poll_recovery_edits_on_control() {
            Ok(batch) if batch.pending_latency_restart.is_none() => return Ok(self),
            Ok(_) => Error::Protocol("native latency keeps changing during reactivation"),
            Err(error) => error,
        };
        Err(self.instance.refuse_epoch_retained(error))
    }

    /// Deactivate/reactivate the same stopped component on its main lane, then
    /// refresh raw facts. This intentionally permits DSP history/tail reset;
    /// ordinary quiescence never does so. The caller must plan compensation
    /// from these new facts before resume/publication. No state is restored or
    /// captured, no buses/setup repeat, and processing remains stopped.
    /// Failure retains the exact original cancelled lease for disposal.
    pub fn reactivate_retained_on_control(
        mut self,
    ) -> std::result::Result<Self, PreparationFailure> {
        let result = (|| {
            self.instance.require_live_epoch()?;
            let operation = self
                .reactivate_sequence
                .checked_add(1)
                .ok_or(Error::Protocol("reactivation operation exhausted"))?;
            let request =
                protocol::reactivate(&self.instance.config, self.instance.sequence, operation)?;
            let until = end_after(self.instance.limits.startup)?;
            let token = self.instance.lease.cancellation.clone();
            self.instance
                .lease
                .channel
                .send(&request, until, &mut || token.check())?;
            self.instance
                .lease
                .channel
                .read(&mut self.instance.packet, until, &mut || token.check())?;
            if self.instance.packet.magic() == b"HXVIER01" {
                return Err(Error::Worker(protocol::error(
                    self.instance.packet.body()?,
                    self.instance.config.binding,
                )?));
            }
            if self.instance.packet.magic() != b"HXVIAR01" {
                return Err(Error::Protocol(
                    "missing component reactivation acknowledgement",
                ));
            }
            let facts = protocol::reactivated_facts(
                self.instance.packet.body()?,
                &self.instance.config,
                self.instance.sequence,
                operation,
            )?;
            self.instance.require_live_epoch()?;
            self.facts = facts;
            self.reactivate_sequence = operation;
            Ok(())
        })();
        match result {
            Ok(()) => Ok(self),
            Err(cause) => Err(self.instance.refuse_epoch_retained(cause)),
        }
    }
    /// Capture component/controller on the helper control lane while the same
    /// actual processor is quiescent. A successful reply grants neither resume
    /// nor publication; startup/canonical envelopes are not replaced. Failure
    /// retains the cancelled original lease for independent disposal, so callers
    /// must keep their last confirmed state and never substitute defaults.
    /// Flush admitted parameter edits first. An edit admitted during this SDK
    /// read refuses its staged snapshot through that same failure custody;
    /// capture never invents a processing/flush call to accept changed state.
    pub fn capture_state_retained_on_control(
        self,
    ) -> std::result::Result<(Self, NativeDeviceState), PreparationFailure> {
        self.capture_state_retained_while_on_control(|| false)
    }

    /// Same exact capture with caller intent cancellation checked in the
    /// existing bounded socket loop. Cancellation retires this original lease;
    /// the predicate owns no helper/endpoint and starts no second waiter.
    /// A capture refusal is terminal here, as for every failure;
    /// callers that keep the plugin running use the refusal-aware capture.
    pub fn capture_state_retained_while_on_control(
        self,
        is_cancelled: impl FnMut() -> bool,
    ) -> std::result::Result<(Self, NativeDeviceState), PreparationFailure> {
        match self.capture_state_or_refusal_retained_while_on_control(is_cancelled)? {
            (stopped, Ok(state)) => Ok((stopped, state)),
            (stopped, Err(refusal)) => Err(stopped
                .instance
                .refuse_epoch_retained(Error::CaptureRefused(refusal))),
        }
    }

    /// Same exact capture, except that a valid helper refusal (state exceeds
    /// the saved profile, or edits kept arriving) is returned with this same
    /// quiescent epoch: nothing was captured, and the caller may capture again,
    /// flush or resume. A malformed or mismatched refusal and every other
    /// failure keep the terminal retained-failure custody described above.
    pub fn capture_state_or_refusal_retained_while_on_control(
        mut self,
        mut is_cancelled: impl FnMut() -> bool,
    ) -> std::result::Result<
        (
            Self,
            std::result::Result<NativeDeviceState, protocol::StateCaptureRefusal>,
        ),
        PreparationFailure,
    > {
        let result = (|| {
            self.instance.require_live_epoch()?;
            let operation = self
                .capture_sequence
                .checked_add(1)
                .ok_or(Error::Protocol("capture operation exhausted"))?;
            let request = protocol::capture_request(
                &self.instance.config,
                self.instance.sequence,
                operation,
            )?;
            let until = end_after(self.instance.limits.startup)?;
            let token = self.instance.lease.cancellation.clone();
            let mut check = || {
                token.check()?;
                if is_cancelled() {
                    Err(Error::Cancelled)
                } else {
                    Ok(())
                }
            };
            self.instance
                .lease
                .channel
                .send(&request, until, &mut check)?;
            self.instance
                .lease
                .channel
                .read(&mut self.instance.packet, until, &mut check)?;
            if self.instance.packet.magic() == b"HXVIER01" {
                return Err(Error::Worker(protocol::error(
                    self.instance.packet.body()?,
                    self.instance.config.binding,
                )?));
            }
            let state = if self.instance.packet.magic() == protocol::CAPTURE_REFUSAL_MAGIC {
                Err(protocol::capture_refusal(
                    self.instance.packet.body()?,
                    &self.instance.config,
                    self.instance.sequence,
                    operation,
                )?)
            } else if self.instance.packet.magic() == b"HXVICS01" {
                Ok(protocol::captured_state(
                    self.instance.packet.body()?,
                    &self.instance.config,
                    self.instance.sequence,
                    operation,
                )?)
            } else {
                return Err(Error::Protocol("missing actual capture acknowledgement"));
            };
            self.instance.require_live_epoch()?;
            check()?;
            self.capture_sequence = operation;
            Ok(state)
        })();
        match result {
            Ok(state) => Ok((self, state)),
            Err(cause) => Err(self.instance.refuse_epoch_retained(cause)),
        }
    }
    /// Flush zero-frame parameter changes on the original stopped SDK worker,
    /// then acknowledge actual controller readback on its main lane. No audio
    /// buffers, callback reentry, PCM prefix advance or canonical mutation occurs.
    /// Any refusal keeps the cancelled original lease in existing failure custody.
    pub fn flush_parameters_retained_on_control(
        self,
        parameters: &crate::transport::Parameters,
    ) -> std::result::Result<(Self, protocol::FlushedParameters), PreparationFailure> {
        self.flush_parameters_retained_while_on_control(parameters, || false)
    }
    /// Caller intent cancellation uses the same bounded channel loop. A late
    /// cancellation retires this actual source; it does not pretend to undo DSP.
    pub fn flush_parameters_retained_while_on_control(
        mut self,
        parameters: &crate::transport::Parameters,
        mut is_cancelled: impl FnMut() -> bool,
    ) -> std::result::Result<(Self, protocol::FlushedParameters), PreparationFailure> {
        let result = (|| {
            self.instance.require_live_epoch()?;
            let operation = self
                .flush_sequence
                .checked_add(1)
                .ok_or(Error::Protocol("flush operation exhausted"))?;
            let request = protocol::flush_request(
                &self.instance.config,
                self.instance.sequence,
                operation,
                parameters,
            )?;
            let until = end_after(self.instance.limits.startup)?;
            let token = self.instance.lease.cancellation.clone();
            let mut check = || {
                token.check()?;
                if is_cancelled() {
                    Err(Error::Cancelled)
                } else {
                    Ok(())
                }
            };
            self.instance
                .lease
                .channel
                .send(&request, until, &mut check)?;
            self.instance
                .lease
                .channel
                .read(&mut self.instance.packet, until, &mut check)?;
            if self.instance.packet.magic() == b"HXVIER01" {
                return Err(Error::Worker(protocol::error(
                    self.instance.packet.body()?,
                    self.instance.config.binding,
                )?));
            }
            if self.instance.packet.magic() != b"HXVIFL01" {
                return Err(Error::Protocol(
                    "missing stopped parameter flush acknowledgement",
                ));
            }
            let values = protocol::flushed_parameters(
                self.instance.packet.body()?,
                &self.instance.config,
                self.instance.sequence,
                operation,
                parameters,
            )?;
            self.instance.require_live_epoch()?;
            check()?;
            self.flush_sequence = operation;
            Ok(values)
        })();
        match result {
            Ok(values) => Ok((self, values)),
            Err(cause) => Err(self.instance.refuse_epoch_retained(cause)),
        }
    }
    pub fn facts(&self) -> Facts {
        self.facts
    }
    pub fn configuration(&self) -> &Configuration {
        &self.instance.config
    }
    /// Same retained native backing while processing is quiescent. Old copied
    /// callback/worker queues belong to their separate joining control owner.
    pub fn retained_storage_on_control(&self) -> Option<PreparedStorage> {
        self.instance.retained_storage_on_control()
    }
    /// Preserve this exact cancelled lease for independent disposal observation.
    /// This is not return of any callback or graph that previously referenced it.
    pub fn cancel_retained_on_control(self) -> PreparationFailure {
        self.instance.cancel_retained_on_control()
    }
    /// Cumulative actual completed blocks, not a graph or callback acceptance.
    pub fn completed_blocks(&self) -> u64 {
        self.instance.sequence
    }
    pub fn cancellation_on_control(&self) -> Cancellation {
        self.instance.cancellation_on_control()
    }
    pub fn child_id_on_control(&self) -> Option<u32> {
        self.instance.child_id_on_control()
    }
    /// Normal final retirement can also consume the stopped instance directly.
    pub fn retire_on_control(mut self) -> Result<Retirement> {
        self.instance.stop_and_reap_on_control()
    }
    pub fn cancel_and_reap_on_control(mut self) -> Result<Retirement> {
        self.instance.cancel_and_reap_on_control()
    }
    /// New binding is only a request. Actual same-thread restart and the exact
    /// native ACK must succeed before the returned owner may exchange PCM.
    /// Source/rate/buses/quantum and cumulative sequence remain unchanged.
    pub fn resume_on_control(
        self,
        next: Binding,
        presentation: Option<PresentationLatency>,
    ) -> Result<Prepared> {
        self.resume_retained_on_control(next, presentation)
            .map_err(PreparationFailure::finish_on_control)
    }
    /// Same actual resumed ACK as the joined form. Every refusal retains the
    /// original cancelled lease in PreparationFailure for control-side polling;
    /// it cannot yield another Quiescent or a reusable Prepared instance.
    pub fn resume_retained_on_control(
        mut self,
        next: Binding,
        presentation: impl Into<protocol::PresentationPlan>,
    ) -> std::result::Result<Prepared, PreparationFailure> {
        let presentation = presentation.into();
        let result = (|| {
            self.instance.require_live_epoch()?;
            if self.instance.parameter_page.is_some() {
                return Err(Error::Protocol("controller page is still pending"));
            }
            presentation.expected(self.facts.presentation_available)?;
            let request = protocol::resume_request(&self.instance.config, next, presentation)?;
            let mut config = self.instance.config.clone();
            config.binding = next;
            let until = end_after(self.instance.limits.startup)?;
            let token = self.instance.lease.cancellation.clone();
            self.instance
                .lease
                .channel
                .send(&request, until, &mut || token.check())?;
            self.instance
                .lease
                .channel
                .read(&mut self.instance.packet, until, &mut || token.check())?;
            if self.instance.packet.magic() == b"HXVIER01" {
                // A refusal can precede binding installation or follow restart.
                // Either actual stamped error is terminal, never resume proof.
                let body = self.instance.packet.body()?;
                let code = protocol::error(body, config.binding)
                    .or_else(|_| protocol::error(body, self.instance.config.binding))?;
                return Err(Error::Worker(code));
            }
            if self.instance.packet.magic() != b"HXVIRR02" {
                return Err(Error::Protocol("missing actual resumed acknowledgement"));
            }
            let applied = protocol::resumed(
                self.instance.packet.body()?,
                &config,
                self.instance.sequence,
                self.facts,
                presentation,
            )?;
            self.instance.require_live_epoch()?;
            Ok((config, applied))
        })();
        match result {
            Ok((config, applied)) => {
                self.instance.config = config;
                self.instance.ready.algorithmic_latency_frames =
                    self.facts.algorithmic_latency_frames;
                self.instance.ready.tail_frames = self.facts.tail_frames;
                self.instance.ready.presentation = applied;
                Ok(self.instance)
            }
            Err(cause) => Err(self.instance.refuse_epoch_retained(cause)),
        }
    }
}
