//! One nonwaiting controller query through the original serialized reader.
//! SDK getters run on the helper main lane while ordinary processing continues.
use super::*;
use protocol::{ControllerRequest, ParameterPageProgress, ParameterPageQuery};

mod automation;
pub use automation::AuthoredParameterMetadata;

#[derive(Clone, Copy)]
pub(super) struct PendingPage {
    operation: u64,
    query: ControllerRequest,
    until: Instant,
}
impl Prepared {
    /// Admit one bounded metadata page or read-only SDK conversion. This exchange returns
    /// pending without waiting for a controller getter. A second begin refuses
    /// without disturbing the first query; rows never identify edit targets.
    pub fn begin_parameter_page_on_worker(&mut self, query: ParameterPageQuery) -> Result<()> {
        self.begin_controller_request_on_worker(query.into())
    }
    /// Includes the editor toggle on the same one-request lane and source.
    pub fn begin_controller_request_on_worker(&mut self, query: ControllerRequest) -> Result<()> {
        self.begin_controller_request_until(query, end_after(self.limits.startup)?)
    }

    fn begin_controller_request_until(
        &mut self,
        query: ControllerRequest,
        until: Instant,
    ) -> Result<()> {
        query.validate()?;
        self.require_live_epoch()?;
        if self.parameter_page.is_some() {
            return Err(Error::Protocol("controller page is still pending"));
        }
        if Instant::now() >= until {
            return Err(Error::Timeout);
        }
        let operation = self
            .parameter_page_sequence
            .checked_add(1)
            .ok_or(Error::Protocol("controller page operation exhausted"))?;
        let pending = PendingPage {
            operation,
            query,
            until,
        };
        match self.parameter_page_exchange(pending, false) {
            Ok(ParameterPageProgress::Pending) => {
                self.parameter_page = Some(pending);
                self.parameter_page_sequence = operation;
                Ok(())
            }
            Ok(_) => self.refuse_parameter_page(Error::Protocol(
                "controller page completed before admission",
            )),
            Err(error) => self.refuse_parameter_page(error),
        }
    }
    /// Poll once on the same reader. Changed is normal invalidation, not a
    /// processor fault. A completed page owns bounded copied data only and
    /// grants no canonical edit, SDK object, source custody or publication.
    pub fn poll_parameter_page_on_worker(&mut self) -> Result<ParameterPageProgress> {
        self.require_live_epoch()?;
        let pending = self
            .parameter_page
            .ok_or(Error::Protocol("controller page is not pending"))?;
        match self.parameter_page_exchange(pending, true) {
            Ok(result) => {
                if !matches!(result, ParameterPageProgress::Pending) {
                    self.parameter_page = None;
                }
                Ok(result)
            }
            Err(error) => self.refuse_parameter_page(error),
        }
    }
    pub(super) fn check_parameter_page_deadline(&self) -> Result<()> {
        if self
            .parameter_page
            .is_some_and(|page| Instant::now() >= page.until)
        {
            return Err(Error::Timeout);
        }
        Ok(())
    }
    fn refuse_parameter_page<T>(&mut self, error: Error) -> Result<T> {
        self.faulted = true;
        self.lease.cancel_native();
        Err(error)
    }
    fn parameter_page_exchange(
        &mut self,
        pending: PendingPage,
        poll: bool,
    ) -> Result<ParameterPageProgress> {
        let request = protocol::parameter_page_request(
            &self.config,
            self.sequence,
            pending.operation,
            pending.query,
            poll,
        )?;
        let until = end_after(self.limits.exchange)?.min(pending.until);
        let token = self.lease.cancellation.clone();
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
        if self.packet.magic() != b"HXVIMD01" {
            return Err(Error::Protocol("missing controller page acknowledgement"));
        }
        let result = protocol::parameter_page(
            self.packet.body()?,
            &self.config,
            self.sequence,
            pending.operation,
            pending.query,
        )?;
        self.require_live_epoch()?;
        Ok(result)
    }
}
impl Quiescent {
    /// Same original stopped reader; querying never invents process/flush work.
    pub fn begin_parameter_page_on_control(&mut self, query: ParameterPageQuery) -> Result<()> {
        self.instance.begin_parameter_page_on_worker(query)
    }
    pub fn poll_parameter_page_on_control(&mut self) -> Result<ParameterPageProgress> {
        self.instance.poll_parameter_page_on_worker()
    }
}
