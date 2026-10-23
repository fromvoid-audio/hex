//! Same loaded child across an acknowledged, serialized processing discontinuity.
//! This does not move an Audio Root, callback endpoint or old queue. The audio
//! owner must quiesce/return those actual carriers before constructing successors.
use super::*;

/// Actual configured child with processing stopped and its completed PCM prefix
/// acknowledged. No PCM exchange API is exposed until a matching resume ACK.
/// Original startup capture is historical; this transition never recaptures or
/// restores state. Component/controller/source custody stays in the same lease.
pub struct Quiescent {
    instance: Prepared,
    facts: Facts,
}
impl Prepared {
    /// Check the actual child and cancellation state before transferring this
    /// unique lease to a worker. This is not graph admission or a future-liveness
    /// guarantee; subsequent exchange failures remain terminal.
    pub fn validate_worker_transfer_on_control(&mut self) -> Result<()> {
        self.require_live_epoch()
    }
    fn require_live_epoch(&mut self) -> Result<()> {
        if self.faulted || self.retired.is_some() {
            return Err(Error::Protocol("instance lease is retired or faulted"));
        }
        self.lease.cancellation.check()?;
        if self.lease.child.observe()?.is_some() {
            return Err(Error::Custody("native epoch owner already exited".into()));
        }
        Ok(())
    }
    fn refuse_epoch(&mut self, cause: Error) -> Error {
        self.faulted = true;
        self.lease.cancel_native();
        let (exit, custody_error) = match self.lease.child.reap() {
            Ok(status) => (Some(status), None),
            Err(error) => (None, Some(error.to_string())),
        };
        Error::Preparation {
            cause: Box::new(cause),
            exit,
            custody_error,
        }
    }
    /// Consume the unique serialized owner after its last completed exchange.
    /// Neither this I/O nor actual child retirement may run on callback.
    pub fn quiesce_on_control(mut self) -> Result<Quiescent> {
        let result = (|| {
            self.require_live_epoch()?;
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
            if self.packet.magic() != b"HXVIQF01" {
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
            }),
            Err(cause) => Err(self.refuse_epoch(cause)),
        }
    }
}
impl Quiescent {
    pub fn facts(&self) -> Facts {
        self.facts
    }
    pub fn configuration(&self) -> &Configuration {
        &self.instance.config
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
        mut self,
        next: Binding,
        presentation: Option<PresentationLatency>,
    ) -> Result<Prepared> {
        let result = (|| {
            self.instance.require_live_epoch()?;
            if self.facts.presentation_available && presentation.is_none() {
                return Err(Error::Protocol(
                    "actual presentation contributions are unavailable",
                ));
            }
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
            if self.instance.packet.magic() != b"HXVIRR01" {
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
                self.instance.ready.presentation = applied;
                Ok(self.instance)
            }
            Err(cause) => Err(self.instance.refuse_epoch(cause)),
        }
    }
}
