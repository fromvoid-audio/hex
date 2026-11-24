//! One copied controller page in the original worker/completion allocation.
//! Fixed worker/shared pages are charged by actual State/Signal constructor
//! sizes. No callback lock, SDK pointer, extra reader or source owner is added.
use super::*;
use hex_plugin_host::{
    Error,
    protocol::{
        ControllerRequest, EditorAdmissionState, EditorSealState, ParameterConversionResult,
        ParameterPage, ParameterPageProgress, PluginEditorState,
    },
};
use std::{
    num::NonZeroU64,
    sync::{TryLockError, Weak},
    time::Instant,
};

mod extension;
mod guarded_write;
pub use extension::NativeWorkerAuthoredParameterProgress;
use extension::{Kind, Operation};
pub use guarded_write::NativeWorkerGuardedEditorWriteProgress;

#[derive(Clone, Copy, Default)]
struct Reply {
    page: ParameterPage,
    changed: bool,
    conversion: Option<ParameterConversionResult>,
    editor: Option<PluginEditorState>,
    editor_seal: Option<EditorSealState>,
    editor_frontier: Option<u64>,
    editor_admission: Option<EditorAdmissionState>,
    extension: Option<NativeWorkerAuthoredParameterProgress>,
    guarded_write: Option<NativeWorkerGuardedEditorWriteProgress>,
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
    routine_read: bool,
}

struct ControllerStart {
    ordinal: u64,
    query: Operation,
}
impl Default for Work {
    fn default() -> Self {
        Self {
            ordinal: None,
            result: None,
            poll_after: Instant::now(),
            extension: false,
            routine_read: false,
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
    kind: Kind,
    parameter_ordinal: Option<NonZeroU64>,
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
    EditorSeal(EditorSealState),
    /// The last editor callback admitted on this query’s original binding.
    EditorFrontier(u64),
    /// Exact producer result for a temporary editor admission hold or release.
    EditorAdmission(EditorAdmissionState),
    Page(Box<ParameterPage>),
    Conversion(Box<ParameterConversionResult>),
    Changed {
        revision: u64,
        total: u16,
    },
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
    pub(super) fn editor_seal_query_pending_on_control(&self) -> Result<bool, &'static str> {
        let slot = self
            .reaper
            .completed
            .capture
            .try_lock()
            .map_err(|error| match error {
                TryLockError::WouldBlock => "native controller mailbox is busy",
                TryLockError::Poisoned(_) => "native controller mailbox failed",
            })?;
        Ok(slot.controller.requested
            && matches!(
                slot.controller.query,
                Operation::Observe(ControllerRequest::SealEditorEdits)
            ))
    }

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
        if matches!(
            query,
            Operation::Observe(ControllerRequest::SealEditorEdits)
        ) && !stopped
        {
            return Err("native editor seal requires a stopped source");
        }
        if stopped {
            if query.kind() == Kind::GuardedWrite {
                return Err("native guarded editor write requires a live worker");
            }
            self.service_stopped_controller()?;
        }
        let mut slot = self
            .reaper
            .completed
            .capture
            .try_lock()
            .map_err(|_| "native controller mailbox is busy")?;
        if query.kind() == Kind::GuardedWrite
            && (slot.capture_pending() || slot.parameter_change.pending())
        {
            return Err("native guarded editor write is busy");
        }
        // Revoked live observations can be superseded after their SDK query
        // completes. An admission terminal can change the same helper's
        // continued editor behavior, so its original token must consume it.
        if stopped
            && slot.controller.requested
            && slot.controller.complete
            && !slot.controller.stopped
            && !slot.controller.changes_editor_admission()
        {
            slot.controller.requested = false;
        }
        if slot.controller.requested {
            return Err("native controller query is already pending");
        }
        let ordinal = slot
            .controller
            .ordinal
            .checked_add(1)
            .ok_or("native controller ordinal exhausted")?;
        let guarded = if let Operation::Guarded(write) = query {
            if let Some(previous) = write.previous
                && slot
                    .parameter_change
                    .guarded_write_readback(previous)
                    .is_none()
            {
                return Err("native guarded editor write was superseded");
            }
            // Supersede both control-side sequenced facts and any callback
            // permit from earlier non-guarded writes before accepting input.
            let next = slot.parameter_change.next_ordinal()?;
            let admission = self
                .reaper
                .control
                .reserve_parameter_input_on_control(next)
                .map_err(|fault| match fault {
                    Fault::Saturated => "native parameter mailbox is busy",
                    Fault::Revoked => "native parameter source is not live",
                    Fault::Exhausted => "native parameter input ordinal exhausted",
                    _ => "native parameter input generation differs",
                })?;
            slot.parameter_change.supersede_with_guarded_write(next);
            Some((next, admission))
        } else {
            None
        };
        let slot = &mut slot.controller;
        slot.ordinal = ordinal;
        slot.query = query;
        slot.requested = true;
        slot.complete = false;
        slot.stopped = stopped;
        let parameter_ordinal = guarded.map(|(ordinal, admission)| {
            admission.publish();
            ordinal
        });
        self.reaper
            .completed
            .controller_requested
            .store(true, Release);
        Ok(NativeWorkerControllerQuery {
            source: Arc::downgrade(&self.reaper.completed),
            binding: self.binding,
            ordinal,
            stopped,
            kind: query.kind(),
            parameter_ordinal,
        })
    }

    /// Only the exact token consumes its terminal once. Revocation wins over
    /// late live observations. Hold and release terminals can still be read on
    /// the joined original worker so the caller can settle the admission change.
    /// The returned page allocation belongs to this control-side caller and is
    /// outside the fixed constructor backing and the audio callback.
    pub fn poll_controller_query_on_control(
        &mut self,
        request: &NativeWorkerControllerQuery,
    ) -> Result<NativeWorkerControllerProgress, &'static str> {
        if request.kind != Kind::Observe {
            return Err("native controller operation kind differs");
        }
        let reply = match self.poll_reply(request)? {
            Poll::Pending => return Ok(NativeWorkerControllerProgress::Pending),
            Poll::Failed => return Ok(NativeWorkerControllerProgress::Failed),
            Poll::Ready(reply) => reply,
        };
        Ok(if let Some(editor) = reply.editor {
            NativeWorkerControllerProgress::Editor(editor)
        } else if let Some(seal) = reply.editor_seal {
            if request.stopped && seal == EditorSealState::Sealed {
                self.editor_edits_sealed = true;
            }
            NativeWorkerControllerProgress::EditorSeal(seal)
        } else if let Some(frontier) = reply.editor_frontier {
            NativeWorkerControllerProgress::EditorFrontier(frontier)
        } else if let Some(admission) = reply.editor_admission {
            NativeWorkerControllerProgress::EditorAdmission(admission)
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
        let changes_editor_admission = {
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
            slot.controller.changes_editor_admission()
        };
        let live = self.transport_is_live_on_control();
        if request.stopped || (!live && changes_editor_admission) {
            match self.service_stopped_controller() {
                Ok(()) => {}
                Err("native preparation is busy") => {
                    return Ok(Poll::Pending);
                }
                Err(_) => return Ok(Poll::Failed),
            }
        } else if !live {
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
        use NativeWorkerGuardedEditorWriteProgress as Guarded;
        if let Some(ordinal) = request.parameter_ordinal
            && let Some(terminal @ (Guarded::Applied | Guarded::ReadbackChanged)) =
                reply.guarded_write
        {
            mailbox
                .parameter_change
                .record_guarded_write(ordinal, terminal == Guarded::Applied);
        }
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
    fn changes_editor_admission(&self) -> bool {
        matches!(
            self.query,
            Operation::Observe(
                ControllerRequest::HoldEditorAdmission(_)
                    | ControllerRequest::ReleaseEditorAdmission(_)
            )
        )
    }

    pub(super) fn guarded_write_pending(&self) -> bool {
        self.requested && self.query.kind() == Kind::GuardedWrite
    }

    fn matches(&self, request: &NativeWorkerControllerQuery) -> bool {
        self.requested
            && self.ordinal == request.ordinal
            && self.stopped == request.stopped
            && self.query.kind() == request.kind
    }
}
fn routine_controller_read(query: Operation) -> bool {
    matches!(
        query,
        Operation::Observe(
            ControllerRequest::Parameters(_)
                | ControllerRequest::SelectedParameters { .. }
                | ControllerRequest::EditorVisibility
        )
    )
}

// Four PCM completions bound catch-up before the next ordinary control turn.
// The existing four-lane rotation still bounds service between eligible lanes.
const PCM_BEFORE_ROUTINE_CONTROL: u8 = 4;

impl State {
    pub(super) fn record_pcm_completion(&mut self) {
        self.pcm_since_routine_control = self
            .pcm_since_routine_control
            .saturating_add(1)
            .min(PCM_BEFORE_ROUTINE_CONTROL);
    }

    pub(super) fn record_empty_pcm_poll(&mut self) {
        self.pcm_since_routine_control = 0;
    }

    // Recheck queued PCM after a sleep and between replies before starting
    // avoidable synchronous I/O. A finite catch-up quota preserves receipt and
    // metadata progress under continuous PCM. Routine periodic polls keep the
    // transport thread's RT policy intact; only heavy non-routine control turns
    // invoke `before_non_routine_control`. Capture, editor admission and writes
    // retain their original ordering, including the offline path.
    pub(super) fn advance_native_control(
        &mut self,
        signal: &Signal,
        before_non_routine_control: impl FnOnce(),
    ) -> Result<(), Error> {
        let allow_routine = signal.offline.is_some()
            || !self.worker.has_queued_request()
            || self.pcm_since_routine_control >= PCM_BEFORE_ROUTINE_CONTROL;
        for offset in 0..4 {
            match (self.control_turn + offset) % 4 {
                0 if (signal.capture_requested.load(Acquire) || self.capture.is_in_flight())
                    && !self.capture.between_polls() =>
                {
                    before_non_routine_control();
                    self.control_turn = 1;
                    return self.advance_capture(signal);
                }
                1 if (signal.controller_requested.load(Acquire)
                    || self.controller.is_in_flight())
                    && !self.controller.between_polls() =>
                {
                    if self.controller.result.is_some() {
                        self.control_turn = 2;
                        return self.advance_controller(signal);
                    }
                    let start = if self.controller.ordinal.is_none() {
                        let Some(start) = self.pending_controller_start(signal)? else {
                            continue;
                        };
                        Some(start)
                    } else {
                        None
                    };
                    let routine = start
                        .as_ref()
                        .map_or(self.controller.routine_read, |start| {
                            routine_controller_read(start.query)
                        });
                    if routine && !allow_routine {
                        continue;
                    }
                    if !routine {
                        before_non_routine_control();
                    }
                    self.control_turn = 2;
                    if routine {
                        self.pcm_since_routine_control = 0;
                    }
                    return self.advance_controller_started(signal, start);
                }
                2 if signal.edits_requested.load(Acquire) => {
                    if self.edits.has_result() {
                        self.control_turn = 3;
                        return self.advance_parameter_edits(signal);
                    }
                    let Some(read) = self.pending_edit_read(signal)? else {
                        continue;
                    };
                    let routine = !read.flush_idle;
                    if routine && !allow_routine {
                        continue;
                    }
                    if !routine {
                        before_non_routine_control();
                    }
                    self.control_turn = 3;
                    if routine {
                        self.pcm_since_routine_control = 0;
                    }
                    return self.advance_parameter_edits_selected(signal, Some(read));
                }
                3 if signal.parameter_change_requested.load(Acquire) => {
                    before_non_routine_control();
                    self.control_turn = 0;
                    return self.advance_idle_parameter_change(signal);
                }
                _ => {}
            }
        }
        Ok(())
    }
    fn pending_controller_start(&self, signal: &Signal) -> Result<Option<ControllerStart>, Error> {
        let slot = match signal.capture.try_lock() {
            Ok(slot) => slot,
            Err(TryLockError::WouldBlock) => return Ok(None),
            Err(TryLockError::Poisoned(_)) => {
                return Err(Error::Protocol("native controller mailbox failed"));
            }
        };
        if !slot.controller.requested || slot.controller.complete {
            return Err(Error::Protocol("native controller request changed"));
        }
        Ok(Some(ControllerStart {
            ordinal: slot.controller.ordinal,
            query: slot.controller.query,
        }))
    }

    pub(super) fn advance_controller(&mut self, signal: &Signal) -> Result<(), Error> {
        self.advance_controller_started(signal, None)
    }

    fn advance_controller_started(
        &mut self,
        signal: &Signal,
        start: Option<ControllerStart>,
    ) -> Result<(), Error> {
        // Read the initial mailbox once. Selection can carry that same exact
        // request through to execution without a second contended lookup.
        let start = if self.controller.ordinal.is_none() {
            if !signal.controller_requested.load(Acquire) {
                return Ok(());
            }
            match start {
                Some(start) => Some(start),
                None => self.pending_controller_start(signal)?,
            }
        } else {
            None
        };
        let Some(Processor::Native(instance)) = self.processor.as_mut() else {
            return Ok(());
        };
        if self.controller.ordinal.is_none() {
            if !signal.controller_requested.load(Acquire) {
                return Ok(());
            }
            let Some(ControllerStart { ordinal, query }) = start else {
                return Ok(());
            };
            self.controller.routine_read = routine_controller_read(query);
            self.controller.result =
                if query.kind() == Kind::GuardedWrite && self.worker.is_revoked() {
                    // Normal reads can drain after revocation. Never start a new
                    // mutation then, even from the stopped controller service.
                    Reply::guarded(NativeWorkerGuardedEditorWriteProgress::Failed)
                } else if query.kind() == Kind::GuardedWrite && self.parameter_change.has_hold() {
                    Reply::guarded(NativeWorkerGuardedEditorWriteProgress::Conflict)
                } else {
                    query.begin(instance)?
                };
            self.controller.extension = query.kind() == Kind::Extension;
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
                    ParameterPageProgress::EditorSeal(seal) => Some(Reply {
                        editor_seal: Some(seal),
                        ..Reply::default()
                    }),
                    ParameterPageProgress::EditorFrontier(frontier) => Some(Reply {
                        editor_frontier: Some(frontier),
                        ..Reply::default()
                    }),
                    ParameterPageProgress::EditorAdmission(admission) => Some(Reply {
                        editor_admission: Some(admission),
                        ..Reply::default()
                    }),
                    ParameterPageProgress::Page(page) => Some(Reply {
                        page: *page,
                        ..Reply::default()
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
                            ..Reply::default()
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

#[cfg(test)]
mod scheduling_tests {
    use super::*;
    use std::cell::Cell;

    // Use the production State selector and actual PCM request/reply queues.
    // The before-control hook observes selection without launching an SDK.
    fn with_state(offline: bool, run: impl FnOnce(&mut State, &Arc<Signal>, &mut Callback)) {
        let (control, mut callback, worker) = channel(
            Binding {
                root: 1,
                instance: 2,
                graph: 3,
                epoch: 4,
            },
            4,
        )
        .unwrap();
        let result = spawn_processor_with(
            control,
            worker,
            Processor::Function(Box::new(Ok)),
            offline,
            |mut guard, signal| {
                run(guard.owner_mut().unwrap(), &signal, &mut callback);
                Err(std::io::Error::other("synchronous scheduling fixture"))
            },
        );
        assert!(result.is_err());
    }

    fn stage(callback: &mut Callback, sequence: u64) {
        let sample = |sequence| [0.25 + sequence as f32 * 0.01; 2];
        let mut output = [[0.0; 2]; 4];
        callback
            .stage(&[sample(sequence); 4], &[], &mut output)
            .unwrap();
        callback.finish(true).unwrap();
        assert_eq!(
            output,
            [sequence.checked_sub(2).map_or([0.0; 2], sample); 4]
        );
    }

    fn request_controller(signal: &Signal, query: ControllerRequest) {
        let mut slot = signal.capture.lock().unwrap();
        slot.controller.query = Operation::Observe(query);
        slot.controller.requested = true;
        signal.controller_requested.store(true, Release);
    }

    #[test]
    fn queued_pcm_precedes_a_routine_controller_poll() {
        with_state(false, |state, signal, callback| {
            stage(callback, 0);
            request_controller(signal, ControllerRequest::default());
            let called = Cell::new(false);
            state
                .advance_native_control(signal, || called.set(true))
                .unwrap();
            assert!(!called.get(), "queued PCM must precede routine native I/O");
            assert_eq!(state.control_turn, 0);
            assert!(signal.controller_requested.load(Acquire));
            assert_eq!(state.worker.receive().unwrap().unwrap().sequence, 0);
        });
    }

    #[test]
    fn wakeup_rechecks_pcm_before_control_even_after_an_earlier_burst() {
        with_state(false, |state, signal, callback| {
            for _ in 0..PCM_BEFORE_ROUTINE_CONTROL {
                state.record_pcm_completion();
            }
            state.record_empty_pcm_poll();
            // A callback publishes PCM while the worker's empty-poll sleep ends.
            stage(callback, 0);
            request_controller(signal, ControllerRequest::EditorVisibility);
            state
                .advance_native_control(signal, || panic!("control overtook wakeup PCM"))
                .unwrap();
            assert_eq!(state.worker.receive().unwrap().unwrap().sequence, 0);
        });
    }

    #[test]
    fn continuous_pcm_keeps_reply_order_and_bounds_routine_control_turns() {
        for capture_pending in [false, true] {
            with_state(false, |state, signal, callback| {
                stage(callback, 0);
                stage(callback, 1);
                request_controller(signal, ControllerRequest::default());
                signal.capture_requested.store(capture_pending, Release);
                let mut routine_turns = Vec::new();
                for sequence in 0..12 {
                    let block = state.worker.receive().unwrap().unwrap();
                    assert_eq!(block.sequence, sequence);
                    state.worker.complete(block).unwrap();
                    state.record_pcm_completion();
                    stage(callback, sequence + 2);
                    assert!(state.worker.has_queued_request());
                    let before_turn = state.control_turn;
                    let demoted = Cell::new(false);
                    state
                        .advance_native_control(signal, || demoted.set(true))
                        .unwrap();
                    if state.control_turn == 2
                        && (before_turn != 2 || state.pcm_since_routine_control == 0)
                    {
                        assert!(
                            !demoted.get(),
                            "routine controller poll must not call deadline.leave()"
                        );
                        routine_turns.push(sequence);
                    } else if capture_pending {
                        assert!(demoted.get());
                    } else {
                        assert!(!demoted.get());
                    }
                }
                assert_eq!(routine_turns, [3, 7, 11]);
                assert!(signal.controller_requested.load(Acquire));
            });
        }
    }

    #[test]
    fn routine_periodic_control_polls_never_demote_transport_deadline() {
        for query in [
            ControllerRequest::default(),
            ControllerRequest::SelectedParameters { first: 0 },
            ControllerRequest::EditorVisibility,
        ] {
            with_state(false, |state, signal, _callback| {
                request_controller(signal, query);
                let demoted = Cell::new(false);
                state
                    .advance_native_control(signal, || demoted.set(true))
                    .unwrap();
                assert_eq!(
                    state.control_turn, 2,
                    "routine query must advance: {query:?}"
                );
                assert!(
                    !demoted.get(),
                    "routine periodic query must not leave RT deadline: {query:?}"
                );
            });
        }
        // Periodic 10 ms parameter touch receipt polls (!flush_idle) also stay in RT.
        // The function processor returns `native edit source missing` after selection.
        with_state(false, |state, signal, _callback| {
            signal.capture.lock().unwrap().edits_pending = true;
            signal.edits_requested.store(true, Release);
            let demoted = Cell::new(false);
            assert!(
                state
                    .advance_native_control(signal, || demoted.set(true))
                    .is_err()
            );
            assert_eq!(state.control_turn, 3);
            assert!(
                !demoted.get(),
                "periodic parameter touch poll must not leave RT deadline"
            );
        });
    }

    #[test]
    fn capture_and_ordering_sensitive_controller_turns_keep_priority() {
        for query in [
            ControllerRequest::SealEditorEdits,
            ControllerRequest::HoldEditorAdmission(NonZeroU64::new(1).unwrap()),
            ControllerRequest::ReleaseEditorAdmission(NonZeroU64::new(1).unwrap()),
            ControllerRequest::CloseEditor,
            ControllerRequest::DetachEditor,
            ControllerRequest::DetachEditorWhenIdle,
            ControllerRequest::ReopenEditor(None),
            ControllerRequest::ReadEditorFrontier,
        ] {
            with_state(false, |state, signal, callback| {
                stage(callback, 0);
                request_controller(signal, query);
                let called = Cell::new(false);
                state
                    .advance_native_control(signal, || called.set(true))
                    .unwrap();
                assert!(
                    called.get(),
                    "ordering-sensitive request was deferred: {query:?}"
                );
            });
        }
        with_state(false, |state, signal, callback| {
            stage(callback, 0);
            signal.capture_requested.store(true, Release);
            let called = Cell::new(false);
            state
                .advance_native_control(signal, || called.set(true))
                .unwrap();
            assert!(called.get());
        });
    }

    #[test]
    fn ordinary_receipts_defer_but_idle_flush_keeps_its_order() {
        with_state(false, |state, signal, callback| {
            stage(callback, 0);
            signal.capture.lock().unwrap().edits_pending = true;
            signal.edits_requested.store(true, Release);
            let called = Cell::new(false);
            state
                .advance_native_control(signal, || called.set(true))
                .unwrap();
            assert!(!called.get());
            signal.capture.lock().unwrap().flush_idle_edits = true;
            // The function processor cannot perform a native flush. Selection
            // must still occur before that explicit missing-source refusal.
            assert!(
                state
                    .advance_native_control(signal, || called.set(true))
                    .is_err()
            );
            assert!(called.get());
        });
    }

    #[test]
    fn busy_unclassified_mailboxes_do_not_demote_or_overtake_queued_pcm() {
        with_state(false, |state, signal, callback| {
            stage(callback, 0);
            request_controller(signal, ControllerRequest::SealEditorEdits);
            signal.edits_requested.store(true, Release);
            let mut mailbox = signal.capture.lock().unwrap();
            mailbox.edits_pending = true;
            state
                .advance_native_control(signal, || panic!("busy mailbox overtook PCM"))
                .unwrap();
            assert_eq!(state.pcm_since_routine_control, 0);
            drop(mailbox);
            // Once its original request is readable, the seal regains its
            // ordering-sensitive priority without waiting for the PCM quota.
            let called = Cell::new(false);
            state
                .advance_native_control(signal, || called.set(true))
                .unwrap();
            assert!(called.get());
            assert_eq!(state.control_turn, 2);
        });
    }

    #[test]
    fn in_flight_controller_retains_its_classification_when_mailbox_is_busy() {
        for routine in [false, true] {
            with_state(false, |state, signal, callback| {
                stage(callback, 0);
                request_controller(signal, ControllerRequest::default());
                state.controller.ordinal = Some(1);
                state.controller.routine_read = routine;
                let _mailbox = signal.capture.lock().unwrap();
                let called = Cell::new(false);
                state
                    .advance_native_control(signal, || called.set(true))
                    .unwrap();
                assert_eq!(called.get(), !routine);
            });
        }
    }

    #[test]
    fn poisoned_controller_mailbox_remains_a_failure() {
        with_state(false, |state, signal, callback| {
            stage(callback, 0);
            request_controller(signal, ControllerRequest::default());
            let poison = Arc::clone(signal);
            assert!(
                std::thread::spawn(move || {
                    let _mailbox = poison.capture.lock().unwrap();
                    panic!("controlled mailbox poison");
                })
                .join()
                .is_err()
            );
            let called = Cell::new(false);
            let error = state.advance_native_control(signal, || called.set(true));
            assert!(matches!(
                error,
                Err(Error::Protocol("native controller mailbox failed"))
            ));
            assert!(!called.get());
        });
    }

    #[test]
    fn offline_control_order_does_not_wait_for_the_live_pcm_quota() {
        with_state(true, |state, signal, callback| {
            stage(callback, 0);
            request_controller(signal, ControllerRequest::default());
            let demoted = Cell::new(false);
            state
                .advance_native_control(signal, || demoted.set(true))
                .unwrap();
            assert_eq!(state.control_turn, 2);
            assert!(!demoted.get());
        });
    }
}
