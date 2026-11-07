//! One copied controller page in the original worker/completion allocation.
//! Fixed worker/shared pages are charged by actual State/Signal constructor
//! sizes. No callback lock, SDK pointer, extra reader or source owner is added.
use super::*;
use hex_plugin_host::{
    Error,
    protocol::{
        ControllerRequest, ParameterConversionResult, ParameterPage, ParameterPageProgress,
        PluginEditorState,
    },
};
use std::{
    sync::{TryLockError, Weak},
    time::Instant,
};

mod extension;
pub use extension::NativeWorkerAuthoredParameterProgress;
use extension::Operation;

#[derive(Clone, Copy, Default)]
struct Reply {
    page: ParameterPage,
    changed: bool,
    conversion: Option<ParameterConversionResult>,
    editor: Option<PluginEditorState>,
    extension: Option<NativeWorkerAuthoredParameterProgress>,
}

#[derive(Default)]
pub(super) struct Slot {
    ordinal: u64,
    query: Operation,
    requested: bool,
    complete: bool,
    stopped: bool,
    reply: Reply,
}
pub(super) struct Work {
    ordinal: Option<u64>,
    result: Option<Reply>,
    poll_after: Instant,
    extension: bool,
}
impl Default for Work {
    fn default() -> Self {
        Self {
            ordinal: None,
            result: None,
            poll_after: Instant::now(),
            extension: false,
        }
    }
}
impl Work {
    pub(super) fn is_in_flight(&self) -> bool {
        self.ordinal.is_some() && self.result.is_none()
    }
    pub(super) fn between_polls(&self) -> bool {
        self.is_in_flight() && Instant::now() < self.poll_after
    }
}

/// Exact original completion cell, binding and ordinal. This weak token owns
/// no native source or endpoint. Retain at most the one current query token.
pub struct NativeWorkerControllerQuery {
    source: Weak<Signal>,
    binding: Binding,
    ordinal: u64,
    stopped: bool,
    extension: bool,
}
impl NativeWorkerControllerQuery {
    /// Whether the original completion allocation has been released. This
    /// does not imply that the source itself was destroyed or return data.
    pub fn original_worker_released_on_control(&self) -> bool {
        self.source.upgrade().is_none()
    }
}
/// Copied controller observations only; never DSP/canonical/publication ACK.
#[derive(Debug)]
pub enum NativeWorkerControllerProgress {
    Pending,
    Editor(PluginEditorState),
    Page(Box<ParameterPage>),
    Conversion(Box<ParameterConversionResult>),
    Changed { revision: u64, total: u16 },
    Failed,
}

// One bounded copied terminal on control. Boxing would allocate a second
// page merely to move it into the public observation result.
#[allow(clippy::large_enum_variant)]
enum Poll {
    Pending,
    Ready(Reply),
    Failed,
}

impl NativeWorker {
    /// Admit one bounded query without SDK/socket work on a live worker.
    /// Full/contended admission preserves its existing query. Stopped mode
    /// first joins the healthy original transport; it never revives playback.
    pub fn begin_controller_query_on_control(
        &mut self,
        query: impl Into<ControllerRequest>,
    ) -> Result<NativeWorkerControllerQuery, &'static str> {
        self.begin_query(Operation::Observe(query.into()))
    }

    fn begin_query(
        &mut self,
        query: Operation,
    ) -> Result<NativeWorkerControllerQuery, &'static str> {
        query.validate()?;
        if self.retirement_requested || !self.retains_original_source_on_control() {
            return Err("native controller source is not live");
        }
        let stopped = !self.transport_is_live_on_control();
        if stopped {
            self.service_stopped_controller()?;
        }
        let mut slot = self
            .reaper
            .completed
            .capture
            .try_lock()
            .map_err(|_| "native controller mailbox is busy")?;
        let slot = &mut slot.controller;
        // A revoked live token is already Failed. Retire its late data only
        // after the actual original SDK query completed on the joined owner.
        if stopped && slot.requested && slot.complete && !slot.stopped {
            slot.requested = false;
        }
        if slot.requested {
            return Err("native controller query is already pending");
        }
        let ordinal = slot
            .ordinal
            .checked_add(1)
            .ok_or("native controller ordinal exhausted")?;
        slot.ordinal = ordinal;
        slot.query = query;
        slot.requested = true;
        slot.complete = false;
        slot.stopped = stopped;
        self.reaper
            .completed
            .controller_requested
            .store(true, Release);
        Ok(NativeWorkerControllerQuery {
            source: Arc::downgrade(&self.reaper.completed),
            binding: self.binding,
            ordinal,
            stopped,
            extension: query.extension(),
        })
    }

    /// Only the exact token consumes its terminal once. Revocation wins over
    /// late live data; stopped queries use the same retained reader after join.
    /// The returned page allocation belongs to this control-side caller and is
    /// outside the fixed constructor backing and the audio callback.
    pub fn poll_controller_query_on_control(
        &mut self,
        request: &NativeWorkerControllerQuery,
    ) -> Result<NativeWorkerControllerProgress, &'static str> {
        if request.extension {
            return Err("native controller operation kind differs");
        }
        let reply = match self.poll_reply(request)? {
            Poll::Pending => return Ok(NativeWorkerControllerProgress::Pending),
            Poll::Failed => return Ok(NativeWorkerControllerProgress::Failed),
            Poll::Ready(reply) => reply,
        };
        Ok(if let Some(editor) = reply.editor {
            NativeWorkerControllerProgress::Editor(editor)
        } else if let Some(conversion) = reply.conversion {
            NativeWorkerControllerProgress::Conversion(Box::new(conversion))
        } else if reply.changed {
            NativeWorkerControllerProgress::Changed {
                revision: reply.page.revision,
                total: reply.page.total,
            }
        } else {
            NativeWorkerControllerProgress::Page(Box::new(reply.page))
        })
    }

    fn poll_reply(&mut self, request: &NativeWorkerControllerQuery) -> Result<Poll, &'static str> {
        if request.binding != self.binding
            || request.source.as_ptr() != Arc::as_ptr(&self.reaper.completed)
        {
            return Err("native controller source differs");
        }
        {
            let slot = match self.reaper.completed.capture.try_lock() {
                Ok(slot) => slot,
                Err(TryLockError::WouldBlock) => {
                    return Ok(Poll::Pending);
                }
                Err(TryLockError::Poisoned(_)) => return Err("native controller mailbox failed"),
            };
            if !slot.controller.matches(request) {
                return Err("native controller operation differs");
            }
        }
        if request.stopped {
            match self.service_stopped_controller() {
                Ok(()) => {}
                Err("native preparation is busy") => {
                    return Ok(Poll::Pending);
                }
                Err(_) => return Ok(Poll::Failed),
            }
        } else if !self.transport_is_live_on_control() {
            return Ok(Poll::Failed);
        }
        let mut mailbox = match self.reaper.completed.capture.try_lock() {
            Ok(slot) => slot,
            Err(TryLockError::WouldBlock) => return Ok(Poll::Pending),
            Err(TryLockError::Poisoned(_)) => return Err("native controller mailbox failed"),
        };
        let slot = &mut mailbox.controller;
        if !slot.matches(request) {
            return Err("native controller operation differs");
        }
        if !slot.complete {
            return Ok(Poll::Pending);
        }
        let reply = slot.reply;
        slot.requested = false;
        drop(mailbox);
        Ok(Poll::Ready(reply))
    }

    fn service_stopped_controller(&mut self) -> Result<(), &'static str> {
        if self.retirement_requested
            || self.transport_is_live_on_control()
            || !self.retains_original_source_on_control()
        {
            return Err("native stopped controller source differs");
        }
        if self
            .reaper
            .join
            .as_ref()
            .is_some_and(|join| !join.is_finished())
        {
            return Err("native preparation is busy");
        }
        self.reaper
            .join_on_control()
            .map_err(|_| "native stopped controller worker failed")?;
        let state = self
            .reaper
            .worker
            .as_mut()
            .ok_or("native stopped controller owner missing")?;
        if state.native_error.is_some() || !matches!(state.processor, Some(Processor::Native(_))) {
            return Err("native stopped controller source failed");
        }
        if let Err(error) = state.advance_controller(&self.reaper.completed) {
            state.native_error = Some(error);
            return Err("native stopped controller query failed");
        }
        Ok(())
    }
}
impl Slot {
    fn matches(&self, request: &NativeWorkerControllerQuery) -> bool {
        self.requested
            && self.ordinal == request.ordinal
            && self.stopped == request.stopped
            && self.query.extension() == request.extension
    }
}
impl State {
    // Three existing control observations share one reader. Rotation bounds
    // service to at most four eligible iterations even when PCM exchanges
    // take longer than the one-millisecond polling interval.
    pub(super) fn advance_native_control(&mut self, signal: &Signal) -> Result<(), Error> {
        for offset in 0..4 {
            match (self.control_turn + offset) % 4 {
                0 if (signal.capture_requested.load(Acquire) || self.capture.is_in_flight())
                    && !self.capture.between_polls() =>
                {
                    self.control_turn = 1;
                    return self.advance_capture(signal);
                }
                1 if (signal.controller_requested.load(Acquire)
                    || self.controller.is_in_flight())
                    && !self.controller.between_polls() =>
                {
                    self.control_turn = 2;
                    return self.advance_controller(signal);
                }
                2 if signal.edits_requested.load(Acquire) => {
                    self.control_turn = 3;
                    return self.advance_parameter_edits(signal);
                }
                3 if signal.parameter_change_requested.load(Acquire) => {
                    self.control_turn = 0;
                    return self.advance_idle_parameter_change(signal);
                }
                _ => {}
            }
        }
        Ok(())
    }
    pub(super) fn advance_controller(&mut self, signal: &Signal) -> Result<(), Error> {
        let Some(Processor::Native(instance)) = self.processor.as_mut() else {
            return Ok(());
        };
        if self.controller.ordinal.is_none() {
            if !signal.controller_requested.load(Acquire) {
                return Ok(());
            }
            let slot = match signal.capture.try_lock() {
                Ok(slot) => slot,
                Err(TryLockError::WouldBlock) => return Ok(()),
                Err(TryLockError::Poisoned(_)) => {
                    return Err(Error::Protocol("native controller mailbox failed"));
                }
            };
            if !slot.controller.requested || slot.controller.complete {
                return Err(Error::Protocol("native controller request changed"));
            }
            let (ordinal, query) = (slot.controller.ordinal, slot.controller.query);
            drop(slot);
            self.controller.result = query.begin(instance)?;
            self.controller.extension = query.extension();
            self.controller.ordinal = Some(ordinal);
        } else if self.controller.result.is_none() {
            if self.controller.between_polls() {
                return Ok(());
            }
            self.controller.result = if self.controller.extension {
                Reply::extension(instance.poll_authored_parameter_extension_on_worker()?)
            } else {
                match instance.poll_parameter_page_on_worker()? {
                    ParameterPageProgress::Pending => None,
                    ParameterPageProgress::Editor(editor) => Some(Reply {
                        editor: Some(editor),
                        ..Reply::default()
                    }),
                    ParameterPageProgress::Page(page) => Some(Reply {
                        page: *page,
                        changed: false,
                        conversion: None,
                        editor: None,
                        extension: None,
                    }),
                    ParameterPageProgress::Conversion(conversion) => Some(Reply {
                        conversion: Some(*conversion),
                        ..Reply::default()
                    }),
                    ParameterPageProgress::Changed { revision, total } => {
                        let mut page = ParameterPage::default();
                        page.revision = revision;
                        page.total = total;
                        Some(Reply {
                            page,
                            changed: true,
                            conversion: None,
                            editor: None,
                            extension: None,
                        })
                    }
                }
            };
        }
        self.controller.poll_after = Instant::now()
            .checked_add(Duration::from_millis(1))
            .ok_or(Error::Protocol("native controller poll time exhausted"))?;
        if let Some(reply) = self.controller.result {
            let mut slot = match signal.capture.try_lock() {
                Ok(slot) => slot,
                Err(TryLockError::WouldBlock) => return Ok(()),
                Err(TryLockError::Poisoned(_)) => {
                    return Err(Error::Protocol("native controller mailbox failed"));
                }
            };
            let slot = &mut slot.controller;
            if Some(slot.ordinal) != self.controller.ordinal || !slot.requested || slot.complete {
                return Err(Error::Protocol("native controller operation changed"));
            }
            slot.reply = reply;
            slot.complete = true;
            signal.controller_requested.store(false, Release);
            self.controller.result = None;
            self.controller.ordinal = None;
        }
        Ok(())
    }
}
