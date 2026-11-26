//! Dedicated out-of-band VST3 control channel, decoupled from the real-time
//! audio PCM socket so AppKit UI stalls and periodic polls never block DSP.
use super::exchange::{self, EditSequenceMode, ExchangeChannel};
use super::*;
use crate::protocol::{ControllerRequest, ParameterPageProgress, ParameterPageQuery};

#[derive(Clone, Copy)]
struct PendingControlPage {
    operation: u64,
    query: ControllerRequest,
    sequence: u64,
    until: Instant,
}

#[derive(Clone, Copy)]
struct PendingControlCapture {
    operation: u64,
    sequence: u64,
    until: Instant,
}

/// Control-plane handle backed by the helper's dedicated non-RT control socket.
/// May stay inside [`Prepared`] for direct callers or move to joining control
/// (`NativeWorker`) while the RT audio worker runs independently.
pub struct ControlClient {
    channel: Channel,
    packet: Packet,
    config: Configuration,
    limits: Limits,
    cancellation: Cancellation,
    faulted: bool,
    observed_blocks: u64,
    edit_query_sequence: u64,
    last_edit_sequence: u64,
    last_edit_frame: u64,
    live_capture: Option<PendingControlCapture>,
    live_capture_sequence: u64,
    parameter_page: Option<PendingControlPage>,
    parameter_page_sequence: u64,
}

impl ControlClient {
    pub(super) fn new(
        channel: Channel,
        packet: Packet,
        config: &Configuration,
        limits: Limits,
        cancellation: Cancellation,
    ) -> Self {
        let mut control_config = config.clone();
        // Never duplicate restored plugin state bytes on the control channel.
        control_config.restore = None;
        Self {
            channel,
            packet,
            config: control_config,
            limits,
            cancellation,
            faulted: false,
            observed_blocks: 0,
            edit_query_sequence: 0,
            last_edit_sequence: 0,
            last_edit_frame: 0,
            live_capture: None,
            live_capture_sequence: 0,
            parameter_page: None,
            parameter_page_sequence: 0,
        }
    }

    pub(super) fn storage_bytes(&self) -> Option<usize> {
        self.config
            .bundle
            .capacity()
            .checked_add(self.config.executable.capacity())?
            .checked_add(self.packet.storage_bytes())
    }

    pub fn is_faulted(&self) -> bool {
        self.faulted
    }

    pub fn bundle_path(&self) -> &str {
        &self.config.bundle
    }

    pub fn has_pending_live_capture(&self) -> bool {
        self.live_capture.is_some()
    }

    pub fn has_pending_controller_request(&self) -> bool {
        self.parameter_page.is_some()
    }

    pub fn cancel_native(&mut self) {
        self.faulted = true;
        self.cancellation.cancel_on_control();
        self.channel.close();
    }

    pub(super) fn sync_after_resume(&mut self, binding: Binding, completed_blocks: u64) {
        self.config.binding = binding;
        self.observed_blocks = self.observed_blocks.max(completed_blocks);
    }

    fn require_live(&self) -> Result<()> {
        self.cancellation.check()?;
        if self.faulted {
            return Err(Error::Protocol("native control channel already faulted"));
        }
        Ok(())
    }

    fn refuse<T>(&mut self, error: Error) -> Result<T> {
        self.cancel_native();
        Err(error)
    }

    fn exchange_io(&mut self) -> ExchangeChannel<'_> {
        ExchangeChannel {
            channel: &mut self.channel,
            packet: &mut self.packet,
            config: &self.config,
            cancellation: &self.cancellation,
            exchange_limit: self.limits.exchange,
        }
    }

    pub fn check_deadlines(&self) -> Result<()> {
        let now = Instant::now();
        if self
            .live_capture
            .is_some_and(|pending| now >= pending.until)
        {
            return Err(Error::Timeout);
        }
        if self
            .parameter_page
            .is_some_and(|pending| now >= pending.until)
        {
            return Err(Error::Timeout);
        }
        Ok(())
    }

    pub fn poll_parameter_edits_on_control(&mut self) -> Result<protocol::ParameterEdits> {
        Ok(self.poll_edits_on_control(false)?.edits)
    }

    pub fn poll_recovery_edits_on_control(&mut self) -> Result<protocol::RecoveryEdits> {
        self.poll_edits_on_control(true)
    }

    fn poll_edits_on_control(&mut self, recovery: bool) -> Result<protocol::RecoveryEdits> {
        let result = (|| {
            self.require_live()?;
            let operation = self
                .edit_query_sequence
                .checked_add(1)
                .ok_or(Error::Protocol("parameter edit operation exhausted"))?;
            let min_sequence = self.observed_blocks;
            let previous = (self.last_edit_sequence, self.last_edit_frame);
            let mut io = self.exchange_io();
            let (result, observed_sequence) = exchange::poll_edits_exchange(
                &mut io,
                EditSequenceMode::AtLeast(min_sequence),
                operation,
                previous,
                recovery,
                false,
            )?;
            self.require_live()?;
            self.observed_blocks = self.observed_blocks.max(observed_sequence);
            self.edit_query_sequence = operation;
            if let Some(last) = result.edits.as_slice().last() {
                self.last_edit_sequence = last.sequence;
                self.last_edit_frame = last.first_frame;
            }
            Ok(result)
        })();
        if result.is_err() {
            self.cancel_native();
        }
        result
    }

    pub fn begin_parameter_page_on_control(&mut self, query: ParameterPageQuery) -> Result<()> {
        self.begin_controller_request_on_control(query.into())
    }

    pub fn begin_controller_request_on_control(&mut self, query: ControllerRequest) -> Result<()> {
        self.begin_controller_request_until(query, end_after(self.limits.startup)?)
    }

    pub(super) fn begin_controller_request_until(
        &mut self,
        query: ControllerRequest,
        until: Instant,
    ) -> Result<()> {
        query.validate()?;
        self.require_live()?;
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
        let pending = PendingControlPage {
            operation,
            query,
            sequence: self.observed_blocks,
            until,
        };
        let mut io = self.exchange_io();
        let outcome = exchange::parameter_page_exchange(
            &mut io,
            pending.sequence,
            pending.operation,
            pending.query,
            pending.until,
            false,
        )
        .and_then(|res| {
            self.require_live()?;
            Ok(res)
        });
        match outcome {
            Ok(ParameterPageProgress::Pending) => {
                self.parameter_page = Some(pending);
                self.parameter_page_sequence = operation;
                Ok(())
            }
            Ok(_) => self.refuse(Error::Protocol(
                "controller page completed before admission",
            )),
            Err(error) => self.refuse(error),
        }
    }

    pub fn poll_parameter_page_on_control(&mut self) -> Result<ParameterPageProgress> {
        self.require_live()?;
        let pending = self
            .parameter_page
            .ok_or(Error::Protocol("controller page is not pending"))?;
        let mut io = self.exchange_io();
        let outcome = exchange::parameter_page_exchange(
            &mut io,
            pending.sequence,
            pending.operation,
            pending.query,
            pending.until,
            true,
        )
        .and_then(|res| {
            self.require_live()?;
            Ok(res)
        });
        match outcome {
            Ok(result) => {
                if !matches!(result, ParameterPageProgress::Pending) {
                    self.parameter_page = None;
                }
                Ok(result)
            }
            Err(error) => self.refuse(error),
        }
    }

    pub fn begin_state_capture_on_control(&mut self) -> Result<()> {
        self.require_live()?;
        if self.live_capture.is_some() {
            return Err(Error::Protocol("native capture is still pending"));
        }
        let operation = self
            .live_capture_sequence
            .checked_add(1)
            .ok_or(Error::Protocol("live capture operation exhausted"))?;
        let pending = PendingControlCapture {
            operation,
            sequence: self.observed_blocks,
            until: end_after(self.limits.startup)?,
        };
        let mut io = self.exchange_io();
        match exchange::live_capture_exchange(
            &mut io,
            pending.sequence,
            pending.operation,
            pending.until,
            false,
        ) {
            Ok(None) => {
                self.live_capture = Some(pending);
                self.live_capture_sequence = operation;
                Ok(())
            }
            Ok(Some(_)) => self.refuse(Error::Protocol("live capture completed before admission")),
            Err(Error::CaptureRefused(refusal)) => {
                self.live_capture_sequence = operation;
                Err(Error::CaptureRefused(refusal))
            }
            Err(cause) => self.refuse(cause),
        }
    }

    pub fn poll_state_capture_on_control(&mut self) -> Result<Option<NativeDeviceState>> {
        self.require_live()?;
        let pending = self
            .live_capture
            .ok_or(Error::Protocol("native capture is not pending"))?;
        let mut io = self.exchange_io();
        match exchange::live_capture_exchange(
            &mut io,
            pending.sequence,
            pending.operation,
            pending.until,
            true,
        ) {
            Ok(state) => {
                if state.is_some() {
                    self.live_capture = None;
                }
                Ok(state)
            }
            Err(Error::CaptureRefused(refusal)) => {
                self.live_capture = None;
                Err(Error::CaptureRefused(refusal))
            }
            Err(cause) => self.refuse(cause),
        }
    }
}
