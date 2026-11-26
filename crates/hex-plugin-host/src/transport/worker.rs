//! Joining-worker transport endpoint for request pickup and reply publication.
use super::*;

impl Worker {
    pub fn binding(&self) -> Binding {
        self.binding
    }
    pub fn quantum(&self) -> u16 {
        self.quantum
    }
    /// Queue-independent revocation by the joining worker owner. Does not
    /// dispose either endpoint or grant native/canonical acceptance.
    pub fn revoke(&self) {
        self.revoked.revoke();
    }
    /// Wake the parked transport worker without allocation or mutexes.
    pub fn wake_worker(&self) {
        self.revoked.wake_worker();
    }
    /// Whether the paired transport has been revoked. Control work which
    /// mutates the processor must check this before taking new SDK work.
    pub fn is_revoked(&self) -> bool {
        self.revoked.is_revoked()
    }
    /// Diagnostic count after matching completion publication, never custody.
    pub fn completed_blocks(&self) -> u64 {
        self.sequence
    }
    /// Observe already-published PCM work without consuming it. This acquire
    /// snapshot grants no admission or exclusion against a later publication.
    /// The original worker remains the only consumer.
    pub fn has_queued_request(&self) -> bool {
        self.requests.slots() != 0
    }
    /// Retained for worker startup callers; wait-free kernel wakeups no longer
    /// require thread-handle registration for `std::thread::Thread::unpark()`.
    pub fn register_wakeup_thread(&self) {}
    /// Park this worker until `timeout`, callback submission, revocation, or an
    /// explicit control wake using a wait-free kernel wakeup primitive (Mach
    /// semaphore on macOS). Re-checks all wake conditions after clearing any
    /// prior notification so no wakeup can be lost.
    pub fn park_timeout(
        &self,
        timeout: std::time::Duration,
        should_stay_awake: impl FnOnce() -> bool,
    ) {
        self.revoked.park_worker_timeout(timeout, || {
            self.requests.slots() != 0 || self.revoked.is_revoked() || should_stay_awake()
        });
    }
    #[cfg(any(test, feature = "test-support"))]
    pub fn pending_requests_for_test(&self) -> usize {
        self.requests.slots()
    }
    pub(super) fn fail<T>(&self, fault: Fault) -> Result<T, Fault> {
        self.revoked.revoke();
        Err(fault)
    }
    /// One nonblocking receive. A revoked lease wins even under queue pressure.
    pub fn receive(&mut self) -> Result<Option<Block>, Fault> {
        self.receive_with_parameter_capacity()
            .map(|request| request.map(|(block, _)| block))
    }
    /// Same original accepted request, with conservative spare input capacity
    /// across its contributing physical callbacks. This budget permits bounded
    /// control input only; it grants no source/publication/canonical authority.
    /// The quantum's Parameters capacity is an independent bound and must
    /// still be checked before adding input.
    pub fn receive_with_parameter_capacity(&mut self) -> Result<Option<(Block, usize)>, Fault> {
        match self.receive_request()? {
            Some(request)
                if request.authored_program.is_some() || request.dense_parameter.is_some() =>
            {
                self.fail(Fault::InvalidParameters)
            }
            Some(request) => Ok(Some((request.block, request.parameter_capacity))),
            None => Ok(None),
        }
    }

    /// Take the original accepted request with its input provenance. An
    /// authored request requires the guarded SDK exchange for this exact source.
    pub fn receive_request(&mut self) -> Result<Option<WorkerRequest>, Fault> {
        if self.revoked.is_revoked() {
            return Err(Fault::Revoked);
        }
        if self.in_flight.is_some() {
            return self.fail(Fault::Protocol);
        }
        let Ok(request) = self.requests.pop() else {
            return Ok(None);
        };
        if request.parameter_capacity > MAX_PARAMETER_POINTS {
            return self.fail(Fault::InvalidParameters);
        }
        let block = request.block;
        if let Err(fault) = block.valid(self.binding, self.quantum, self.sequence) {
            return self.fail(fault);
        }
        let dense_parameter = match self.dense_requests.as_mut() {
            Some(requests) => match dense::receive(requests, &block) {
                Ok(lane) => lane,
                Err(fault) => return self.fail(fault),
            },
            None => None,
        };
        if let Err(fault) = self
            .revoked
            .record_received_program(self.sequence, request.authored_program)
        {
            return self.fail(fault);
        }
        self.in_flight = Some(InFlight {
            manual_origin: request.manual_origin,
            editor_scope: request.editor_scope,
        });
        Ok(Some(WorkerRequest {
            block,
            sidechain: request.sidechain,
            parameter_capacity: request.parameter_capacity,
            dense_parameter,
            authored_program: request.authored_program,
            manual_origin_available: request.manual_origin.is_some(),
            editor_scope: request.editor_scope,
        }))
    }
    /// Publish exactly one matching reply after the isolated processor finishes.
    /// Revocation discards a late completion; it cannot undo DSP already run.
    pub fn complete(&mut self, reply: Block) -> Result<(), Fault> {
        self.complete_with_manual_parameter(reply, None)
    }

    /// Attach a validated offset-zero host operation to this exact successful
    /// PCM reply. Callers use the original joining worker's input receipt.
    pub fn complete_with_manual_parameter(
        &mut self,
        reply: Block,
        manual: Option<ManualParameterInput>,
    ) -> Result<(), Fault> {
        self.complete_with_parameter_observations(reply, manual, None)
    }

    /// Attach the validated result of the opted-in SDK exchange for this
    /// in-flight request. An empty batch still witnesses its physical quantum;
    /// absence means the SDK exchange did not opt in.
    pub fn complete_with_parameter_observations(
        &mut self,
        reply: Block,
        manual: Option<ManualParameterInput>,
        performs: Option<EditorPerforms>,
    ) -> Result<(), Fault> {
        self.complete_with_editor_facts(
            reply,
            manual,
            performs.map(|performs| EditorPcmFacts {
                performs,
                restorations: Default::default(),
                lost_origin: false,
            }),
        )
    }

    /// Publish the distinct SDK restoration beside the same checked PCM.
    /// The later Main owner still controls final output acceptance.
    pub fn complete_with_editor_facts(
        &mut self,
        reply: Block,
        manual: Option<ManualParameterInput>,
        facts: Option<EditorPcmFacts>,
    ) -> Result<(), Fault> {
        if self.revoked.is_revoked() {
            return Err(Fault::Revoked);
        }
        let Some(in_flight) = self.in_flight.take() else {
            return self.fail(Fault::Protocol);
        };
        self.revoked
            .processed_prefix
            .store(self.sequence.saturating_add(1), Release);
        let editor = match (in_flight.editor_scope, facts) {
            (None, None) => None,
            (Some(scope), Some(facts)) => Some(EditorExchange {
                scope,
                facts,
                origin: in_flight.manual_origin,
            }),
            _ => return self.fail(Fault::Protocol),
        };
        let reply = OutputReply {
            block: reply,
            editor,
            manual: manual.map(|input| ManualExchange {
                input,
                origin: in_flight.manual_origin,
            }),
        };
        if let Err(fault) = reply.valid(self.binding, self.quantum, self.sequence) {
            return self.fail(fault);
        }
        let Some(next) = self.sequence.checked_add(1) else {
            return self.fail(Fault::Exhausted);
        };
        if self.replies.push(reply).is_err() {
            return self.fail(Fault::Saturated);
        }
        self.sequence = next;
        Ok(())
    }
}
